//! Danmu statistics calculation.
//!
//! Provides statistics aggregation for danmu messages during a session.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::LazyLock;

use super::message::{DanmuMessage, DanmuType};

/// Statistics for a danmu collection session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DanmuStatistics {
    /// Total number of danmu messages received
    pub total_count: u64,
    /// Number of chat messages
    pub chat_count: u64,
    /// Number of gift messages
    pub gift_count: u64,
    /// Approximate number of distinct senders (HyperLogLog estimate, ~1% error)
    pub unique_talkers: u64,
    /// Top talkers (user_id -> message count)
    pub top_talkers: Vec<TopTalker>,
    /// Top gift senders (`message_count` holds total gift items, not messages)
    pub top_gifters: Vec<TopTalker>,
    /// Most-sent gifts by name (total items across the session)
    pub top_gifts: Vec<GiftCount>,
    /// Word frequency (word -> count)
    pub word_frequency: Vec<WordFrequency>,
    /// Danmu rate timeseries (timestamp -> count)
    pub rate_timeseries: Vec<RateDataPoint>,
    /// Session start time
    pub start_time: Option<DateTime<Utc>>,
    /// Session end time
    pub end_time: Option<DateTime<Utc>>,
    /// Duration in seconds
    pub duration_secs: u64,
}

/// A top talker entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopTalker {
    pub user_id: String,
    pub username: String,
    pub message_count: u64,
}

/// A word frequency entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordFrequency {
    pub word: String,
    pub count: u64,
}

/// A gift tally entry (gift name -> total items sent).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GiftCount {
    pub name: String,
    pub count: u64,
}

/// A rate timeseries data point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateDataPoint {
    pub timestamp: DateTime<Utc>,
    pub count: u64,
}

/// Fixed-size HyperLogLog for estimating distinct senders.
///
/// 2^12 registers (4 KiB) give a standard error of ~1.6%, which is plenty for
/// an engagement metric; the alternative (an exact `HashSet<String>`) grows
/// unbounded in high-churn rooms, which the heavy-hitter structures in this
/// module deliberately avoid.
#[derive(Debug, Clone)]
struct HyperLogLog {
    registers: Vec<u8>,
}

impl HyperLogLog {
    /// log2 of the register count.
    const P: u32 = 12;

    fn new() -> Self {
        Self {
            registers: vec![0; 1 << Self::P],
        }
    }

    fn insert(&mut self, value: &str) {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        value.hash(&mut hasher);
        let hash = hasher.finish();
        // High P bits pick the register; the rank is the position of the
        // first set bit in the remaining bits.
        let index = (hash >> (64 - Self::P)) as usize;
        let rest = hash << Self::P;
        let rank = (rest.leading_zeros() + 1).min(64 - Self::P + 1) as u8;
        if rank > self.registers[index] {
            self.registers[index] = rank;
        }
    }

    fn estimate(&self) -> u64 {
        let m = self.registers.len() as f64;
        let sum: f64 = self.registers.iter().map(|&r| (-(r as f64)).exp2()).sum();
        let alpha = 0.7213 / (1.0 + 1.079 / m);
        let raw = alpha * m * m / sum;

        // Linear-counting correction for small cardinalities.
        if raw <= 2.5 * m {
            let zeros = self.registers.iter().filter(|&&r| r == 0).count();
            if zeros > 0 {
                return (m * (m / zeros as f64).ln()).round() as u64;
            }
        }
        raw.round() as u64
    }
}

/// Bounded gift-name tally (Space-Saving eviction).
///
/// Platform gift catalogs are small (hundreds of names at most), so the
/// capacity is rarely hit; the eviction path only guards against a
/// misbehaving provider flooding unique names.
#[derive(Debug, Clone)]
struct BoundedTally {
    capacity: usize,
    counts: HashMap<String, u64>,
}

impl BoundedTally {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            counts: HashMap::new(),
        }
    }

    fn add(&mut self, key: &str, n: u64) {
        if let Some(count) = self.counts.get_mut(key) {
            *count = count.saturating_add(n);
            return;
        }
        if self.counts.len() < self.capacity {
            self.counts.insert(key.to_string(), n);
            return;
        }
        let min_entry = self
            .counts
            .iter()
            .min_by_key(|(_, count)| **count)
            .map(|(key, count)| (key.clone(), *count));
        if let Some((min_key, min_count)) = min_entry {
            self.counts.remove(&min_key);
            self.counts
                .insert(key.to_string(), min_count.saturating_add(n));
        }
    }

    fn top_n(&self, n: usize) -> Vec<GiftCount> {
        let mut entries: Vec<_> = self.counts.iter().collect();
        entries.sort_by(|(ak, a), (bk, b)| b.cmp(a).then_with(|| ak.cmp(bk)));
        entries.truncate(n);
        entries
            .into_iter()
            .map(|(name, count)| GiftCount {
                name: name.clone(),
                count: *count,
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
struct TalkerCounter {
    username: String,
    count: u64,
    error: u64,
}

#[derive(Debug, Clone)]
struct TalkerHeavyHitters {
    capacity: usize,
    counters: HashMap<String, TalkerCounter>,
}

impl TalkerHeavyHitters {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            counters: HashMap::new(),
        }
    }

    fn increment(&mut self, user_id: &str, username: &str) {
        self.increment_by(user_id, username, 1);
    }

    fn increment_by(&mut self, user_id: &str, username: &str, n: u64) {
        if let Some(counter) = self.counters.get_mut(user_id) {
            counter.count = counter.count.saturating_add(n);
            if counter.username != username {
                counter.username = username.to_string();
            }
            return;
        }

        if self.counters.len() < self.capacity {
            self.counters.insert(
                user_id.to_string(),
                TalkerCounter {
                    username: username.to_string(),
                    count: n,
                    error: 0,
                },
            );
            return;
        }

        let min_key_and_count = self
            .counters
            .iter()
            .min_by_key(|(_, counter)| counter.count)
            .map(|(key, counter)| (key.clone(), counter.count));

        if let Some((key, min_count)) = min_key_and_count {
            self.counters.remove(&key);
            self.counters.insert(
                user_id.to_string(),
                TalkerCounter {
                    username: username.to_string(),
                    count: min_count.saturating_add(n),
                    error: min_count,
                },
            );
        }
    }

    fn top_n(&self, n: usize) -> Vec<TopTalker> {
        if n == 0 || self.counters.is_empty() {
            return Vec::new();
        }

        let mut entries: Vec<_> = self.counters.iter().collect();
        entries.sort_by(|(aid, a), (bid, b)| {
            b.count
                .cmp(&a.count)
                .then_with(|| aid.cmp(bid))
                .then_with(|| a.error.cmp(&b.error))
        });
        entries.truncate(n);
        entries
            .into_iter()
            .map(|(user_id, counter)| TopTalker {
                user_id: user_id.clone(),
                username: counter.username.clone(),
                message_count: counter.count,
            })
            .collect()
    }

    fn into_top_n(self, n: usize) -> Vec<TopTalker> {
        if n == 0 || self.counters.is_empty() {
            return Vec::new();
        }

        let mut entries: Vec<_> = self.counters.into_iter().collect();
        entries.sort_by(|(aid, a), (bid, b)| {
            b.count
                .cmp(&a.count)
                .then_with(|| aid.cmp(bid))
                .then_with(|| a.error.cmp(&b.error))
        });
        entries.truncate(n);
        entries
            .into_iter()
            .map(|(user_id, counter)| TopTalker {
                user_id,
                username: counter.username,
                message_count: counter.count,
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
struct WordCounter {
    count: u64,
    error: u64,
}

#[derive(Debug, Clone)]
struct CountMinSketch {
    width: usize,
    depth: usize,
    rows: Vec<Vec<u64>>,
}

impl CountMinSketch {
    fn new(width: usize, depth: usize) -> Self {
        let width = width.max(64);
        let depth = depth.max(2);
        let mut rows = Vec::with_capacity(depth);
        for _ in 0..depth {
            rows.push(vec![0; width]);
        }
        Self { width, depth, rows }
    }

    fn hash_with_seed(value: &str, seed: u64) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        seed.hash(&mut hasher);
        value.hash(&mut hasher);
        hasher.finish()
    }

    fn increment(&mut self, value: &str, inc: u64) {
        for i in 0..self.depth {
            let hash = Self::hash_with_seed(value, i as u64);
            let index = (hash as usize) % self.width;
            self.rows[i][index] = self.rows[i][index].saturating_add(inc);
        }
    }

    fn estimate(&self, value: &str) -> u64 {
        let mut min_value = u64::MAX;
        for i in 0..self.depth {
            let hash = Self::hash_with_seed(value, i as u64);
            let index = (hash as usize) % self.width;
            min_value = min_value.min(self.rows[i][index]);
        }
        min_value
    }
}

#[derive(Debug, Clone)]
struct WordHeavyHitters {
    capacity: usize,
    counters: HashMap<String, WordCounter>,
    sketch: Option<CountMinSketch>,
}

impl WordHeavyHitters {
    fn new(capacity: usize, sketch: Option<CountMinSketch>) -> Self {
        Self {
            capacity: capacity.max(1),
            counters: HashMap::new(),
            sketch,
        }
    }

    fn increment(&mut self, word: &str) {
        if let Some(sketch) = &mut self.sketch {
            sketch.increment(word, 1);
        }

        if let Some(counter) = self.counters.get_mut(word) {
            counter.count = counter.count.saturating_add(1);
            return;
        }

        if self.counters.len() < self.capacity {
            self.counters
                .insert(word.to_string(), WordCounter { count: 1, error: 0 });
            return;
        }

        let min_key_and_count = self
            .counters
            .iter()
            .min_by_key(|(_, counter)| counter.count)
            .map(|(key, counter)| (key.clone(), counter.count));

        if let Some((key, min_count)) = min_key_and_count {
            self.counters.remove(&key);
            let cms_count = self
                .sketch
                .as_ref()
                .map(|sketch| sketch.estimate(word))
                .unwrap_or(0);
            let count = min_count.saturating_add(1).max(cms_count);
            self.counters.insert(
                word.to_string(),
                WordCounter {
                    count,
                    error: min_count,
                },
            );
        }
    }

    fn score(&self, key: &str, counter: &WordCounter) -> u64 {
        if let Some(sketch) = &self.sketch {
            counter.count.max(sketch.estimate(key))
        } else {
            counter.count
        }
    }

    fn compare_entries(&self, a: (&String, &WordCounter), b: (&String, &WordCounter)) -> Ordering {
        self.score(a.0, a.1)
            .cmp(&self.score(b.0, b.1))
            .reverse()
            .then_with(|| a.0.cmp(b.0))
            .then_with(|| a.1.error.cmp(&b.1.error))
    }

    fn top_n(&self, n: usize) -> Vec<WordFrequency> {
        if n == 0 || self.counters.is_empty() {
            return Vec::new();
        }

        let mut entries: Vec<_> = self.counters.iter().collect();
        entries.sort_by(|a, b| self.compare_entries(*a, *b));
        entries.truncate(n);
        entries
            .into_iter()
            .map(|(word, counter)| WordFrequency {
                word: word.clone(),
                count: self.score(word, counter),
            })
            .collect()
    }

    fn into_top_n(self, n: usize) -> Vec<WordFrequency> {
        if n == 0 || self.counters.is_empty() {
            return Vec::new();
        }

        let sketch = self.sketch;
        let score = |word: &str, counter: &WordCounter| {
            if let Some(sketch) = &sketch {
                counter.count.max(sketch.estimate(word))
            } else {
                counter.count
            }
        };

        let mut entries: Vec<_> = self.counters.into_iter().collect();
        entries.sort_by(|(aw, a), (bw, b)| {
            score(aw, a)
                .cmp(&score(bw, b))
                .reverse()
                .then_with(|| aw.cmp(bw))
                .then_with(|| a.error.cmp(&b.error))
        });
        entries.truncate(n);
        entries
            .into_iter()
            .map(|(word, counter)| WordFrequency {
                count: score(&word, &counter),
                word,
            })
            .collect()
    }
}

/// Aggregator for calculating danmu statistics.
#[derive(Debug)]
pub struct StatisticsAggregator {
    /// Total message count
    total_count: u64,
    /// Chat message count
    chat_count: u64,
    /// Gift message count
    gift_count: u64,
    /// Heavy hitters for active talkers (Space-Saving).
    talker_hh: TalkerHeavyHitters,
    /// Heavy hitters for gift senders, weighted by gift items.
    gifter_hh: TalkerHeavyHitters,
    /// Tally of gift names, weighted by gift items.
    gift_tally: BoundedTally,
    /// Distinct-sender estimator.
    unique_hll: HyperLogLog,
    /// Heavy hitters for words (Space-Saving + optional Count-Min Sketch).
    word_hh: WordHeavyHitters,
    /// Rate data points.
    rate_data: VecDeque<RateDataPoint>,
    /// Current rate bucket
    current_bucket: Option<(DateTime<Utc>, u64)>,
    /// Bucket duration in seconds
    bucket_duration_secs: u64,
    /// Session start time
    start_time: Option<DateTime<Utc>>,
    /// Maximum number of top talkers to track
    max_top_talkers: usize,
    /// Maximum number of words to return.
    max_words: usize,
    /// Maximum number of rate points kept in memory.
    max_rate_points: usize,
    /// Stop words to filter out
    stop_words: &'static HashSet<&'static str>,
}

static STOP_WORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(default_stop_words);

impl StatisticsAggregator {
    /// Maximum number of gift-name entries returned by snapshots.
    const MAX_TOP_GIFTS: usize = 20;
    /// Internal capacity for the gift-name tally.
    const GIFT_TALLY_CAPACITY: usize = 128;

    /// Create a new statistics aggregator.
    pub fn new() -> Self {
        Self::with_config(10, 50, 10)
    }

    /// Create a new statistics aggregator with custom configuration.
    pub fn with_config(
        max_top_talkers: usize,
        max_words: usize,
        bucket_duration_secs: u64,
    ) -> Self {
        let talker_capacity = max_top_talkers.max(1).saturating_mul(8);
        let word_capacity = max_words.max(1).saturating_mul(4);
        let cms_width = (word_capacity.saturating_mul(32))
            .next_power_of_two()
            .max(256);
        let cms_depth = 4;
        let max_rate_points = ((6 * 60 * 60) / bucket_duration_secs.max(1) as usize).max(60);
        Self {
            total_count: 0,
            chat_count: 0,
            gift_count: 0,
            talker_hh: TalkerHeavyHitters::new(talker_capacity),
            gifter_hh: TalkerHeavyHitters::new(talker_capacity),
            gift_tally: BoundedTally::new(Self::GIFT_TALLY_CAPACITY),
            unique_hll: HyperLogLog::new(),
            word_hh: WordHeavyHitters::new(
                word_capacity,
                Some(CountMinSketch::new(cms_width, cms_depth)),
            ),
            rate_data: VecDeque::new(),
            current_bucket: None,
            bucket_duration_secs,
            start_time: None,
            max_top_talkers,
            max_words,
            max_rate_points,
            stop_words: &STOP_WORDS,
        }
    }

    /// Record a message.
    ///
    /// Gift and super-chat messages (`DanmuType::Gift`/`SuperChat`) update the
    /// gifter/gift tallies from the `gift_name`/`gift_count` metadata keys set
    /// by the platform providers; everything else feeds the word statistics.
    pub fn record_message(&mut self, message: &DanmuMessage) {
        let timestamp = message.timestamp;

        // Set start time on first message
        if self.start_time.is_none() {
            self.start_time = Some(timestamp);
        }

        // Update counts
        let is_gift = matches!(message.message_type, DanmuType::Gift | DanmuType::SuperChat);
        self.total_count += 1;
        if is_gift {
            self.gift_count += 1;
        } else {
            self.chat_count += 1;
        }

        self.talker_hh
            .increment(&message.user_id, &message.username);
        self.unique_hll.insert(&message.user_id);

        if is_gift {
            // `gift_count` metadata = items in this batch; SuperChat and
            // providers without batch info count as one item.
            let items = message
                .metadata
                .as_ref()
                .and_then(|meta| meta.get("gift_count"))
                .and_then(|value| value.as_u64())
                .unwrap_or(1)
                .max(1);
            self.gifter_hh
                .increment_by(&message.user_id, &message.username, items);
            if let Some(name) = message
                .metadata
                .as_ref()
                .and_then(|meta| meta.get("gift_name"))
                .and_then(|value| value.as_str())
            {
                self.gift_tally.add(name, items);
            }
        } else if !message.content.is_empty() {
            // Update word counts (only for chat messages)
            self.process_words(&message.content);
        }

        // Update rate data
        self.update_rate_bucket(timestamp);
    }

    /// Process words from a message.
    ///
    /// Words are maximal alphanumeric runs: splitting on `!is_alphanumeric()`
    /// treats whitespace, ASCII *and* full-width CJK punctuation (`,`, `。`,
    /// `!`, ...) and symbols/emoji as separators, so a Chinese message like
    /// `加油,主播!` counts `加油` and `主播` instead of one giant token.
    fn process_words(&mut self, content: &str) {
        for word in content
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
        {
            let word_lower = word.to_lowercase();

            // Skip stop words and very short words
            if word_lower.len() < 2 || self.stop_words.contains(word_lower.as_str()) {
                continue;
            }

            self.word_hh.increment(&word_lower);
        }
    }

    /// Update the rate bucket.
    fn update_rate_bucket(&mut self, timestamp: DateTime<Utc>) {
        let bucket_start = self.get_bucket_start(timestamp);

        match &mut self.current_bucket {
            Some((start, count)) if *start == bucket_start => {
                *count += 1;
            }
            Some((start, count)) => {
                // Save current bucket and start new one
                self.rate_data.push_back(RateDataPoint {
                    timestamp: *start,
                    count: *count,
                });
                if self.rate_data.len() > self.max_rate_points {
                    self.coarsen_rate_data();
                }
                // Recompute: `coarsen_rate_data` doubles `bucket_duration_secs`,
                // which changes the bucket alignment for new messages.
                let bucket_start = self.get_bucket_start(timestamp);
                self.current_bucket = Some((bucket_start, 1));
            }
            None => {
                self.current_bucket = Some((bucket_start, 1));
            }
        }
    }

    /// Halve the rate-timeseries resolution in place: double
    /// `bucket_duration_secs` and re-bucket every stored point under the new
    /// alignment, merging counts that land in the same bucket.
    ///
    /// Long sessions therefore keep full-session coverage with a bounded
    /// number of points, instead of silently dropping the oldest buckets.
    fn coarsen_rate_data(&mut self) {
        self.bucket_duration_secs = self.bucket_duration_secs.saturating_mul(2);
        let old = std::mem::take(&mut self.rate_data);
        for point in old {
            let bucket_start = self.get_bucket_start(point.timestamp);
            match self.rate_data.back_mut() {
                Some(last) if last.timestamp == bucket_start => {
                    last.count = last.count.saturating_add(point.count);
                }
                _ => self.rate_data.push_back(RateDataPoint {
                    timestamp: bucket_start,
                    count: point.count,
                }),
            }
        }
    }

    /// Get the bucket start time for a timestamp.
    fn get_bucket_start(&self, timestamp: DateTime<Utc>) -> DateTime<Utc> {
        let secs = timestamp.timestamp();
        let bucket_secs =
            (secs / self.bucket_duration_secs as i64) * self.bucket_duration_secs as i64;
        DateTime::from_timestamp(bucket_secs, 0).unwrap_or(timestamp)
    }

    /// Finalize and return statistics.
    pub fn finalize(mut self, end_time: DateTime<Utc>) -> DanmuStatistics {
        // Flush current bucket
        if let Some((start, count)) = self.current_bucket.take() {
            self.rate_data.push_back(RateDataPoint {
                timestamp: start,
                count,
            });
        }

        // Calculate duration
        let duration_secs = self
            .start_time
            .map(|start| (end_time - start).num_seconds().max(0) as u64)
            .unwrap_or(0);

        let top_talkers = self.talker_hh.into_top_n(self.max_top_talkers);
        let top_gifters = self.gifter_hh.into_top_n(self.max_top_talkers);
        let word_frequency = self.word_hh.into_top_n(self.max_words);
        DanmuStatistics {
            total_count: self.total_count,
            chat_count: self.chat_count,
            gift_count: self.gift_count,
            unique_talkers: self.unique_hll.estimate(),
            top_talkers,
            top_gifters,
            top_gifts: self.gift_tally.top_n(Self::MAX_TOP_GIFTS),
            word_frequency,
            rate_timeseries: self.rate_data.into_iter().collect(),
            start_time: self.start_time,
            end_time: Some(end_time),
            duration_secs,
        }
    }

    /// Get current statistics without finalizing.
    pub fn current_stats(&self) -> DanmuStatistics {
        let top_talkers = self.talker_hh.top_n(self.max_top_talkers);
        let top_gifters = self.gifter_hh.top_n(self.max_top_talkers);
        let word_frequency = self.word_hh.top_n(self.max_words);

        let mut rate_data: Vec<_> = self.rate_data.iter().cloned().collect();
        if let Some((start, count)) = &self.current_bucket {
            rate_data.push(RateDataPoint {
                timestamp: *start,
                count: *count,
            });
        }

        DanmuStatistics {
            total_count: self.total_count,
            chat_count: self.chat_count,
            gift_count: self.gift_count,
            unique_talkers: self.unique_hll.estimate(),
            top_talkers,
            top_gifters,
            top_gifts: self.gift_tally.top_n(Self::MAX_TOP_GIFTS),
            word_frequency,
            rate_timeseries: rate_data,
            start_time: self.start_time,
            end_time: None,
            duration_secs: 0,
        }
    }
}

impl Default for StatisticsAggregator {
    fn default() -> Self {
        Self::new()
    }
}

/// Default stop words for filtering.
fn default_stop_words() -> HashSet<&'static str> {
    let words = [
        // English
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "may", "might", "must", "shall",
        "can", "need", "dare", "to", "of", "in", "for", "on", "with", "at", "by", "from", "as",
        "into", "through", "during", "before", "after", "above", "below", "between", "under",
        "again", "further", "then", "once", "here", "there", "when", "where", "why", "how", "all",
        "each", "few", "more", "most", "other", "some", "such", "no", "nor", "not", "only", "own",
        "same", "so", "than", "too", "very", "just", "and", "but", "if", "or", "because", "until",
        "while", "this", "that", "these", "those", "it", "its", "he", "she", "they", "them", "his",
        "her", "their", "what", "which", "who", "whom", // Chinese common words
        "的", "了", "是", "在", "我", "有", "和", "就", "不", "人", "都", "一", "一个", "上", "也",
        "很", "到", "说", "要", "去", "你", "会", "着", "没有", "看", "好", "自己", "这", "那",
        // Common chat expressions
        "lol", "lmao", "haha", "hehe", "xd", "gg", "ez", "wp", "666", "233", "哈哈", "呵呵", "嘿嘿",
    ];

    words.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::time::Instant;

    fn chat_at(
        user_id: &str,
        username: &str,
        content: &str,
        timestamp: DateTime<Utc>,
    ) -> DanmuMessage {
        DanmuMessage::chat("test-id", user_id, username, content).with_timestamp(timestamp)
    }

    fn gift_at(
        user_id: &str,
        username: &str,
        gift_name: &str,
        gift_count: u32,
        timestamp: DateTime<Utc>,
    ) -> DanmuMessage {
        DanmuMessage::gift("test-id", user_id, username, gift_name, gift_count)
            .with_timestamp(timestamp)
    }

    #[test]
    fn test_record_message() {
        let mut agg = StatisticsAggregator::new();
        let now = Utc::now();

        agg.record_message(&chat_at("user1", "User One", "Hello world!", now));
        agg.record_message(&chat_at("user2", "User Two", "Hi there!", now));
        agg.record_message(&chat_at("user1", "User One", "Another message", now));

        let stats = agg.current_stats();
        assert_eq!(stats.total_count, 3);
        assert_eq!(stats.chat_count, 3);
        assert_eq!(stats.gift_count, 0);
        assert_eq!(stats.unique_talkers, 2);
    }

    #[test]
    fn test_top_talkers() {
        let mut agg = StatisticsAggregator::with_config(3, 10, 10);
        let now = Utc::now();

        // User1: 5 messages, User2: 3 messages, User3: 1 message
        for _ in 0..5 {
            agg.record_message(&chat_at("user1", "User One", "msg", now));
        }
        for _ in 0..3 {
            agg.record_message(&chat_at("user2", "User Two", "msg", now));
        }
        agg.record_message(&chat_at("user3", "User Three", "msg", now));

        let stats = agg.current_stats();
        assert_eq!(stats.top_talkers.len(), 3);
        assert_eq!(stats.top_talkers[0].user_id, "user1");
        assert_eq!(stats.top_talkers[0].message_count, 5);
        assert_eq!(stats.top_talkers[1].user_id, "user2");
        assert_eq!(stats.top_talkers[1].message_count, 3);
    }

    #[test]
    fn test_word_frequency() {
        let mut agg = StatisticsAggregator::with_config(10, 10, 10);
        let now = Utc::now();

        agg.record_message(&chat_at("user1", "User", "hello world hello", now));
        agg.record_message(&chat_at("user2", "User", "hello rust world", now));

        let stats = agg.current_stats();

        // "hello" should appear 3 times, "world" 2 times, "rust" 1 time
        let hello = stats.word_frequency.iter().find(|w| w.word == "hello");
        assert!(hello.is_some());
        assert_eq!(hello.unwrap().count, 3);
    }

    #[test]
    fn test_rate_timeseries() {
        let mut agg = StatisticsAggregator::with_config(10, 10, 10);
        let base = Utc.with_ymd_and_hms(2024, 1, 1, 12, 0, 0).unwrap();

        // Messages in first bucket
        agg.record_message(&chat_at("user1", "User", "msg1", base));
        agg.record_message(&chat_at(
            "user1",
            "User",
            "msg2",
            base + chrono::Duration::seconds(5),
        ));

        // Messages in second bucket
        agg.record_message(&chat_at(
            "user1",
            "User",
            "msg3",
            base + chrono::Duration::seconds(15),
        ));

        let end_time = base + chrono::Duration::seconds(20);
        let stats = agg.finalize(end_time);

        assert_eq!(stats.rate_timeseries.len(), 2);
        assert_eq!(stats.rate_timeseries[0].count, 2); // First bucket
        assert_eq!(stats.rate_timeseries[1].count, 1); // Second bucket
    }

    /// Full-width CJK punctuation must separate words the same way ASCII
    /// punctuation does, and emoji/symbols must not glue tokens together.
    #[test]
    fn test_word_frequency_splits_on_cjk_punctuation() {
        let mut agg = StatisticsAggregator::with_config(10, 10, 10);
        let now = Utc::now();

        agg.record_message(&chat_at("u1", "User", "加油,主播!加油", now));
        agg.record_message(&chat_at("u2", "User", "主播。牛逼🤣牛逼", now));

        let stats = agg.current_stats();
        let count_of = |w: &str| {
            stats
                .word_frequency
                .iter()
                .find(|entry| entry.word == w)
                .map(|entry| entry.count)
        };

        assert_eq!(count_of("加油"), Some(2));
        assert_eq!(count_of("主播"), Some(2));
        assert_eq!(count_of("牛逼"), Some(2));
        // No token should contain the full-width separators.
        assert!(
            stats
                .word_frequency
                .iter()
                .all(|entry| !entry.word.contains([',', '!', '。'])),
            "full-width punctuation must not appear inside tokens: {:?}",
            stats.word_frequency
        );
    }

    /// When the number of rate buckets exceeds `max_rate_points`, resolution is
    /// halved instead of dropping the oldest points: the first bucket's
    /// timestamp must survive and the total count must be preserved.
    #[test]
    fn test_rate_timeseries_coarsens_instead_of_dropping() {
        let mut agg = StatisticsAggregator::with_config(10, 10, 10);
        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();

        // One message per 10s bucket, spanning well past max_rate_points buckets.
        let buckets = agg.max_rate_points as i64 + 100;
        for i in 0..buckets {
            agg.record_message(&chat_at(
                "user1",
                "User",
                "msg",
                base + chrono::Duration::seconds(i * 10),
            ));
        }

        let end_time = base + chrono::Duration::seconds(buckets * 10);
        let max_points = agg.max_rate_points;
        let stats = agg.finalize(end_time);

        assert!(
            stats.rate_timeseries.len() <= max_points + 1,
            "rate points must stay bounded (got {})",
            stats.rate_timeseries.len()
        );
        let total: u64 = stats.rate_timeseries.iter().map(|p| p.count).sum();
        assert_eq!(total, buckets as u64, "coarsening must not lose counts");
        assert_eq!(
            stats.rate_timeseries.first().map(|p| p.timestamp),
            Some(base),
            "earliest bucket must survive coarsening"
        );
    }

    #[test]
    fn test_finalize() {
        let mut agg = StatisticsAggregator::new();
        let start = Utc.with_ymd_and_hms(2024, 1, 1, 12, 0, 0).unwrap();
        let end = start + chrono::Duration::minutes(30);

        agg.record_message(&chat_at("user1", "User", "Hello", start));

        let stats = agg.finalize(end);

        assert_eq!(stats.start_time, Some(start));
        assert_eq!(stats.end_time, Some(end));
        assert_eq!(stats.duration_secs, 1800); // 30 minutes
    }

    #[test]
    fn test_gift_counting() {
        let mut agg = StatisticsAggregator::new();
        let now = Utc::now();

        agg.record_message(&chat_at("user1", "User", "chat", now));
        agg.record_message(&gift_at("user2", "User", "Rocket", 1, now));
        agg.record_message(&gift_at("user3", "User", "Rocket", 1, now));

        let stats = agg.current_stats();
        assert_eq!(stats.total_count, 3);
        assert_eq!(stats.chat_count, 1);
        assert_eq!(stats.gift_count, 2);
    }

    /// Gifters are ranked by total gift items (`gift_count` metadata), not by
    /// message count, and gift names are tallied the same way. SuperChat
    /// messages carry no gift metadata and must count as one item.
    #[test]
    fn test_gift_aggregation() {
        let mut agg = StatisticsAggregator::new();
        let now = Utc::now();

        // whale: one message of 10 rockets; regular: three single flowers.
        agg.record_message(&gift_at("whale", "Whale", "Rocket", 10, now));
        for _ in 0..3 {
            agg.record_message(&gift_at("regular", "Regular", "Flower", 1, now));
        }
        agg.record_message(
            &DanmuMessage::super_chat("sc-1", "sc_user", "SC User", "hello", 30)
                .with_timestamp(now),
        );

        let stats = agg.current_stats();
        assert_eq!(stats.gift_count, 5);

        assert_eq!(stats.top_gifters[0].user_id, "whale");
        assert_eq!(stats.top_gifters[0].message_count, 10);
        assert_eq!(stats.top_gifters[1].user_id, "regular");
        assert_eq!(stats.top_gifters[1].message_count, 3);
        assert_eq!(stats.top_gifters[2].user_id, "sc_user");
        assert_eq!(stats.top_gifters[2].message_count, 1);

        let rocket = stats.top_gifts.iter().find(|g| g.name == "Rocket");
        assert_eq!(rocket.map(|g| g.count), Some(10));
        let flower = stats.top_gifts.iter().find(|g| g.name == "Flower");
        assert_eq!(flower.map(|g| g.count), Some(3));

        // Gift messages must not pollute word frequency.
        assert!(
            stats.word_frequency.iter().all(|w| w.word != "rocket"),
            "gift content must not be tokenized into words"
        );
    }

    /// The HyperLogLog estimate must be close to the true cardinality for
    /// both tiny (exact via linear counting) and larger sender sets.
    #[test]
    fn test_unique_talkers_estimate() {
        let mut agg = StatisticsAggregator::new();
        let now = Utc::now();

        let total_users = 5_000usize;
        for i in 0..total_users {
            let user_id = format!("user-{i}");
            // Every user sends two messages; uniques must not double.
            agg.record_message(&chat_at(&user_id, "User", "msg", now));
            agg.record_message(&chat_at(&user_id, "User", "msg again", now));
        }

        let estimate = agg.current_stats().unique_talkers as f64;
        let error = (estimate - total_users as f64).abs() / total_users as f64;
        assert!(
            error < 0.05,
            "HLL estimate {estimate} deviates {:.1}% from {total_users}",
            error * 100.0
        );
    }

    #[test]
    fn test_heavy_hitter_high_cardinality_bounds() {
        let mut agg = StatisticsAggregator::with_config(10, 50, 10);
        let now = Utc::now();
        let total_messages = 50_000usize;

        let start = Instant::now();
        for i in 0..total_messages {
            // Very high-cardinality users.
            let user_id = format!("user-{}", i % 20_000);
            let username = format!("User{}", i % 20_000);
            // Very high-cardinality words mixed with hot keywords.
            let content = format!("word{} hot hot", i % 30_000);
            agg.record_message(&chat_at(&user_id, &username, &content, now));
        }
        let elapsed = start.elapsed();

        // Internal heavy-hitter structures must stay bounded.
        assert!(agg.talker_hh.counters.len() <= agg.talker_hh.capacity);
        assert!(agg.word_hh.counters.len() <= agg.word_hh.capacity);
        assert_eq!(agg.total_count as usize, total_messages);

        // Public outputs are also bounded by config.
        let stats = agg.current_stats();
        assert!(stats.top_talkers.len() <= 10);
        assert!(stats.word_frequency.len() <= 50);

        // Non-failing signal for local profiling/regression checks.
        eprintln!(
            "high_cardinality_bounds: messages={} elapsed_ms={}",
            total_messages,
            elapsed.as_millis()
        );
    }
}
