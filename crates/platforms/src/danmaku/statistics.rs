//! Danmu statistics calculation.
//!
//! Provides statistics aggregation for danmu messages during a session.

use chrono::{DateTime, Utc};
use jieba_rs::Jieba;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::LazyLock;

use super::message::{DanmuMessage, DanmuType};

/// Livestream vocabulary absent from jieba's general-Chinese dictionary that its
/// HMM also does not infer, so without seeding it the words split into single
/// characters and `StatisticsAggregator::record_word` discards them as noise.
///
/// A seed, not an exhaustive list: anything missing still segments per-character
/// and simply goes uncounted rather than producing a wrong word.
const DANMU_DICT_WORDS: &[&str] = &[
    "牛逼",
    "弹幕",
    "下播",
    "开播",
    "直播间",
    "老铁",
    "破防",
    "泪目",
    "上分",
    "团战",
    "打野",
    "中单",
    "野区",
    "残血",
    "反杀",
    "秒了",
    "抬走",
    "整活",
];

/// Shared segmenter for CJK word frequency.
///
/// `Jieba` is immutable after construction and `cut` takes `&self`, so one
/// instance serves every concurrent session. `LazyLock` keeps the embedded
/// dictionary out of memory entirely for deployments whose danmu is never CJK.
static JIEBA: LazyLock<Jieba> = LazyLock::new(|| {
    let mut jieba = Jieba::new();
    for word in DANMU_DICT_WORDS {
        jieba.add_word(word, None, None);
    }
    jieba
});

/// Whether `text` contains a character that needs dictionary segmentation.
///
/// Covers CJK Unified Ideographs (plus the common extension A), Japanese
/// hiragana/katakana, and the CJK compatibility block. Hangul is excluded:
/// Korean is written with spaces, so the outer split already tokenizes it.
fn contains_cjk(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c,
            '\u{3040}'..='\u{30FF}'      // hiragana + katakana
            | '\u{3400}'..='\u{4DBF}'    // CJK ext. A
            | '\u{4E00}'..='\u{9FFF}'    // CJK unified ideographs
            | '\u{F900}'..='\u{FAFF}'    // CJK compatibility ideographs
        )
    })
}

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
    /// Width of one `rate_timeseries` bucket.
    ///
    /// Not fixed for the session: `StatisticsAggregator::coarsen_rate_data`
    /// doubles it whenever the point count would exceed its bound, so consumers
    /// must read this rather than assume the configured value.
    #[serde(default)]
    pub bucket_duration_secs: u64,
}

/// A top talker entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopTalker {
    pub user_id: String,
    pub username: String,
    pub message_count: u64,
    /// Space-Saving overestimate bound: the true count is at least
    /// `message_count - error`. Zero means the count is exact.
    #[serde(default)]
    pub error: u64,
}

/// A word frequency entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordFrequency {
    pub word: String,
    pub count: u64,
    /// Space-Saving overestimate bound: the true count is at least
    /// `count - error`. Zero means the count is exact.
    #[serde(default)]
    pub error: u64,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

    /// Drop all but the `capacity` highest entries, for a checkpoint restored
    /// under a smaller configured capacity.
    fn shrink_to_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(1);
        if self.counts.len() <= self.capacity {
            return;
        }
        let mut entries: Vec<_> = std::mem::take(&mut self.counts).into_iter().collect();
        entries.sort_by(|(ak, a), (bk, b)| b.cmp(a).then_with(|| ak.cmp(bk)));
        entries.truncate(self.capacity);
        self.counts = entries.into_iter().collect();
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TalkerCounter {
    username: String,
    count: u64,
    error: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

    /// Drop all but the `capacity` highest counters, for a checkpoint restored
    /// under a smaller configured capacity.
    fn shrink_to_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(1);
        if self.counters.len() <= self.capacity {
            return;
        }
        let mut entries: Vec<_> = std::mem::take(&mut self.counters).into_iter().collect();
        entries.sort_by(|(aid, a), (bid, b)| {
            b.count
                .cmp(&a.count)
                .then_with(|| aid.cmp(bid))
                .then_with(|| a.error.cmp(&b.error))
        });
        entries.truncate(self.capacity);
        self.counters = entries.into_iter().collect();
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
                error: counter.error,
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
                error: counter.error,
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WordCounter {
    count: u64,
    error: u64,
}

/// Bounded word tally (Space-Saving eviction).
///
/// `WordCounter::error` is the overestimate an evicted-and-readmitted word may
/// carry, bounded by `total_increments / capacity`; `count - error` is a lower
/// bound on the true frequency. Snapshots report both so consumers can tell an
/// exact count from an approximate one.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WordHeavyHitters {
    capacity: usize,
    counters: HashMap<String, WordCounter>,
}

impl WordHeavyHitters {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            counters: HashMap::new(),
        }
    }

    fn increment(&mut self, word: &str) {
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
            self.counters.insert(
                word.to_string(),
                WordCounter {
                    count: min_count.saturating_add(1),
                    error: min_count,
                },
            );
        }
    }

    fn compare_entries(a: (&str, &WordCounter), b: (&str, &WordCounter)) -> Ordering {
        b.1.count
            .cmp(&a.1.count)
            .then_with(|| a.0.cmp(b.0))
            .then_with(|| a.1.error.cmp(&b.1.error))
    }

    /// Drop all but the `capacity` highest counters, for a checkpoint restored
    /// under a smaller configured capacity.
    fn shrink_to_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(1);
        if self.counters.len() <= self.capacity {
            return;
        }
        let mut entries: Vec<_> = std::mem::take(&mut self.counters).into_iter().collect();
        entries.sort_by(|a, b| Self::compare_entries((&a.0, &a.1), (&b.0, &b.1)));
        entries.truncate(self.capacity);
        self.counters = entries.into_iter().collect();
    }

    fn top_n(&self, n: usize) -> Vec<WordFrequency> {
        if n == 0 || self.counters.is_empty() {
            return Vec::new();
        }

        let mut entries: Vec<_> = self.counters.iter().collect();
        entries.sort_by(|a, b| Self::compare_entries((a.0, a.1), (b.0, b.1)));
        entries.truncate(n);
        entries
            .into_iter()
            .map(|(word, counter)| WordFrequency {
                word: word.clone(),
                count: counter.count,
                error: counter.error,
            })
            .collect()
    }

    fn into_top_n(self, n: usize) -> Vec<WordFrequency> {
        if n == 0 || self.counters.is_empty() {
            return Vec::new();
        }

        let mut entries: Vec<_> = self.counters.into_iter().collect();
        entries.sort_by(|a, b| Self::compare_entries((&a.0, &a.1), (&b.0, &b.1)));
        entries.truncate(n);
        entries
            .into_iter()
            .map(|(word, counter)| WordFrequency {
                word,
                count: counter.count,
                error: counter.error,
            })
            .collect()
    }
}

/// Tuning for [`StatisticsAggregator`].
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

/// A checkpoint of an aggregator's internal state.
///
/// Exists so a collector restarted mid-session can continue counting instead of
/// beginning at zero: the published `DanmuStatistics` cannot be reloaded into an
/// aggregator, because a HyperLogLog estimate does not yield back its registers
/// and a truncated top-N does not yield back the counters behind it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatorState {
    /// Layout version. A checkpoint written by a different version is discarded
    /// rather than migrated, so collection degrades to starting fresh.
    pub version: u32,
    total_count: u64,
    chat_count: u64,
    gift_count: u64,
    talker_hh: TalkerHeavyHitters,
    gifter_hh: TalkerHeavyHitters,
    gift_tally: BoundedTally,
    unique_hll: HyperLogLog,
    word_hh: WordHeavyHitters,
    rate_data: VecDeque<RateDataPoint>,
    current_bucket: Option<(DateTime<Utc>, u64)>,
    bucket_duration_secs: u64,
    start_time: Option<DateTime<Utc>>,
}

impl AggregatorState {
    /// Current layout version. Bump on any change to the fields above or to the
    /// meaning of the structures they hold.
    pub const VERSION: u32 = 1;

    /// Messages counted when this checkpoint was taken, for logging a resume.
    pub fn total_count(&self) -> u64 {
        self.total_count
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
    /// Heavy hitters for words (Space-Saving).
    word_hh: WordHeavyHitters,
    /// Rate data points.
    rate_data: VecDeque<RateDataPoint>,
    /// Current rate bucket
    current_bucket: Option<(DateTime<Utc>, u64)>,
    /// Current bucket width; doubled by `coarsen_rate_data`.
    bucket_duration_secs: u64,
    /// Session start time
    start_time: Option<DateTime<Utc>>,
    /// Maximum number of rate points kept in memory.
    max_rate_points: usize,
    /// Reporting sizes, capacities and stop-word additions.
    config: StatisticsConfig,
    /// Stop words to filter out
    stop_words: &'static HashSet<&'static str>,
}

static STOP_WORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(default_stop_words);

impl StatisticsAggregator {
    /// Create a new statistics aggregator with default tuning.
    pub fn new() -> Self {
        Self::with_settings(StatisticsConfig::default())
    }

    /// Create a new statistics aggregator, overriding only the reported sizes and
    /// the bucket width. Capacities keep their defaults.
    pub fn with_config(
        max_top_talkers: usize,
        max_words: usize,
        bucket_duration_secs: u64,
    ) -> Self {
        Self::with_settings(StatisticsConfig {
            top_talkers: max_top_talkers,
            top_words: max_words,
            rate_bucket_secs: bucket_duration_secs,
            ..StatisticsConfig::default()
        })
    }

    /// Create a new statistics aggregator from a full configuration.
    pub fn with_settings(config: StatisticsConfig) -> Self {
        let max_rate_points = Self::max_rate_points_for(config.rate_bucket_secs);
        Self {
            total_count: 0,
            chat_count: 0,
            gift_count: 0,
            talker_hh: TalkerHeavyHitters::new(config.talker_capacity),
            gifter_hh: TalkerHeavyHitters::new(config.talker_capacity),
            gift_tally: BoundedTally::new(config.gift_capacity),
            unique_hll: HyperLogLog::new(),
            word_hh: WordHeavyHitters::new(config.word_capacity),
            rate_data: VecDeque::new(),
            current_bucket: None,
            bucket_duration_secs: config.rate_bucket_secs.max(1),
            start_time: None,
            max_rate_points,
            config,
            stop_words: &STOP_WORDS,
        }
    }

    /// Point budget for the rate timeseries: enough to cover six hours at the
    /// configured width before `coarsen_rate_data` halves the resolution.
    fn max_rate_points_for(bucket_duration_secs: u64) -> usize {
        ((6 * 60 * 60) / bucket_duration_secs.max(1) as usize).max(60)
    }

    /// Take a checkpoint of everything needed to continue counting later.
    pub fn export_state(&self) -> AggregatorState {
        AggregatorState {
            version: AggregatorState::VERSION,
            total_count: self.total_count,
            chat_count: self.chat_count,
            gift_count: self.gift_count,
            talker_hh: self.talker_hh.clone(),
            gifter_hh: self.gifter_hh.clone(),
            gift_tally: self.gift_tally.clone(),
            unique_hll: self.unique_hll.clone(),
            word_hh: self.word_hh.clone(),
            rate_data: self.rate_data.clone(),
            current_bucket: self.current_bucket,
            bucket_duration_secs: self.bucket_duration_secs,
            start_time: self.start_time,
        }
    }

    /// Rebuild an aggregator from a checkpoint, under a possibly-changed `config`.
    ///
    /// Returns `None` for a checkpoint whose `version` this build does not
    /// understand, so a layout change makes collection start fresh rather than
    /// fail. Counter maps larger than the configured capacity are trimmed to
    /// their highest entries, since the configuration may have shrunk between
    /// runs; the rate timeseries keeps the width it was recorded at, because
    /// re-bucketing stored points under a new width would misplace them.
    pub fn from_state(state: AggregatorState, config: StatisticsConfig) -> Option<Self> {
        if state.version != AggregatorState::VERSION {
            return None;
        }

        let mut talker_hh = state.talker_hh;
        let mut gifter_hh = state.gifter_hh;
        let mut word_hh = state.word_hh;
        let mut gift_tally = state.gift_tally;
        talker_hh.shrink_to_capacity(config.talker_capacity);
        gifter_hh.shrink_to_capacity(config.talker_capacity);
        word_hh.shrink_to_capacity(config.word_capacity);
        gift_tally.shrink_to_capacity(config.gift_capacity);

        let bucket_duration_secs = state.bucket_duration_secs.max(1);
        Some(Self {
            total_count: state.total_count,
            chat_count: state.chat_count,
            gift_count: state.gift_count,
            talker_hh,
            gifter_hh,
            gift_tally,
            unique_hll: state.unique_hll,
            word_hh,
            rate_data: state.rate_data,
            current_bucket: state.current_bucket,
            bucket_duration_secs,
            start_time: state.start_time,
            max_rate_points: Self::max_rate_points_for(bucket_duration_secs),
            config,
            stop_words: &STOP_WORDS,
        })
    }

    /// Messages recorded so far, for callers deciding whether a snapshot or
    /// checkpoint would carry anything new.
    pub fn total_count(&self) -> u64 {
        self.total_count
    }

    /// Whether `word` should be excluded from word frequency.
    fn is_stop_word(&self, word: &str) -> bool {
        self.stop_words.contains(word)
            || (!self.config.extra_stop_words.is_empty()
                && self
                    .config
                    .extra_stop_words
                    .iter()
                    .any(|entry| entry == word))
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
    /// Two passes. The outer split on `!is_alphanumeric()` treats whitespace,
    /// ASCII *and* full-width punctuation (`,`, `。`, `!`, ...) and
    /// symbols/emoji as separators. That alone is not enough for CJK: ideographs
    /// *are* alphanumeric and CJK text has no spaces, so `主播今天好厉害啊`
    /// survives the split as one token. Runs containing CJK are therefore passed
    /// through `JIEBA`, while Latin-script runs keep the allocation-free path.
    fn process_words(&mut self, content: &str) {
        for run in content
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
        {
            if contains_cjk(run) {
                // HMM inference on: the bundled dictionary is general Chinese, and
                // without it a common word like `主播` splits into `主` + `播`,
                // which `record_word` then discards as single-character noise.
                // `Token::word` borrows from `run`, so segments cost no allocation
                // beyond the returned Vec.
                for token in JIEBA.cut(run, true) {
                    self.record_word(token.word);
                }
            } else {
                self.record_word(run);
            }
        }
    }

    /// Count one candidate word, after case folding and stop-word filtering.
    fn record_word(&mut self, word: &str) {
        // `chars().count()`, not `len()`: a byte length keeps single CJK
        // characters (3 bytes) while dropping single ASCII ones.
        if word.chars().count() < 2 {
            return;
        }

        // Borrow when there is no case to fold, which is every CJK word.
        let word_lower = if word.chars().any(char::is_uppercase) {
            Cow::Owned(word.to_lowercase())
        } else {
            Cow::Borrowed(word)
        };

        if self.is_stop_word(&word_lower) {
            return;
        }

        self.word_hh.increment(&word_lower);
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

        let top_talkers = self.talker_hh.into_top_n(self.config.top_talkers);
        let top_gifters = self.gifter_hh.into_top_n(self.config.top_talkers);
        let word_frequency = self.word_hh.into_top_n(self.config.top_words);
        DanmuStatistics {
            total_count: self.total_count,
            chat_count: self.chat_count,
            gift_count: self.gift_count,
            unique_talkers: self.unique_hll.estimate(),
            top_talkers,
            top_gifters,
            top_gifts: self.gift_tally.top_n(self.config.top_gifts),
            word_frequency,
            rate_timeseries: self.rate_data.into_iter().collect(),
            start_time: self.start_time,
            end_time: Some(end_time),
            duration_secs,
            bucket_duration_secs: self.bucket_duration_secs,
        }
    }

    /// Get current statistics without finalizing.
    ///
    /// `end_time` stays `None` because the session is still running, but
    /// `duration_secs` is the elapsed time so far so consumers can compute a
    /// per-minute rate from a live snapshot the same way they do from a final one.
    pub fn current_stats(&self) -> DanmuStatistics {
        let top_talkers = self.talker_hh.top_n(self.config.top_talkers);
        let top_gifters = self.gifter_hh.top_n(self.config.top_talkers);
        let word_frequency = self.word_hh.top_n(self.config.top_words);

        let mut rate_data: Vec<_> = self.rate_data.iter().cloned().collect();
        if let Some((start, count)) = &self.current_bucket {
            rate_data.push(RateDataPoint {
                timestamp: *start,
                count: *count,
            });
        }

        let duration_secs = self
            .start_time
            .map(|start| (Utc::now() - start).num_seconds().max(0) as u64)
            .unwrap_or(0);

        DanmuStatistics {
            total_count: self.total_count,
            chat_count: self.chat_count,
            gift_count: self.gift_count,
            unique_talkers: self.unique_hll.estimate(),
            top_talkers,
            top_gifters,
            top_gifts: self.gift_tally.top_n(self.config.top_gifts),
            word_frequency,
            rate_timeseries: rate_data,
            start_time: self.start_time,
            end_time: None,
            duration_secs,
            bucket_duration_secs: self.bucket_duration_secs,
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

    /// Word counts must stay close to the truth in a long, high-cardinality
    /// session. A Count-Min Sketch used to back the word tally, and `score`
    /// returned `max(counter, sketch_estimate)`; the sketch's collision floor is
    /// `total_increments / width`, so every reported count was lifted to it —
    /// a word sent three times reported ~1500 and ranked second, with the rest of
    /// the top list filled by count-1 noise at the same fabricated value.
    #[test]
    fn word_counts_are_not_inflated_by_collisions() {
        // A deliberately small capacity so eviction runs constantly; the error
        // bound's meaning does not depend on the scale.
        let mut agg = StatisticsAggregator::with_settings(StatisticsConfig {
            top_words: 50,
            word_capacity: 64,
            ..StatisticsConfig::default()
        });
        let now = Utc::now();

        let noise = 20_000usize;
        for i in 0..noise {
            agg.record_message(&chat_at("u1", "User", &format!("noise{i} hotword"), now));
        }
        for _ in 0..3 {
            agg.record_message(&chat_at("u1", "User", "rarelyseen", now));
        }

        let stats = agg.current_stats();
        let hotword = stats
            .word_frequency
            .iter()
            .find(|entry| entry.word == "hotword")
            .expect("the hot word must be tracked");
        assert_eq!(
            hotword.count, noise as u64,
            "an always-resident word must be counted exactly"
        );

        // `rarelyseen` may or may not survive eviction, but if it is reported the
        // count must be near the truth rather than a collision floor.
        if let Some(rare) = stats
            .word_frequency
            .iter()
            .find(|entry| entry.word == "rarelyseen")
        {
            assert!(
                rare.count - rare.error <= 3,
                "reported count {} with error bound {} implies a true count above 3",
                rare.count,
                rare.error
            );
        }
    }

    /// While fewer distinct keys than the configured capacity have been seen,
    /// Space-Saving never evicts, so counts are exact and carry no error bound.
    /// This is why capacity is configured separately from the reported size.
    #[test]
    fn counts_are_exact_below_capacity() {
        let mut agg = StatisticsAggregator::with_settings(StatisticsConfig {
            top_talkers: 5,
            talker_capacity: 64,
            ..StatisticsConfig::default()
        });
        let now = Utc::now();

        for i in 0..32 {
            let user = format!("user-{i}");
            for _ in 0..=i {
                agg.record_message(&chat_at(&user, "User", "msg", now));
            }
        }

        let stats = agg.current_stats();
        assert_eq!(
            stats.top_talkers.len(),
            5,
            "output size is the reported size"
        );
        assert_eq!(
            stats.top_talkers[0].user_id, "user-31",
            "the busiest talker ranks first"
        );
        assert_eq!(stats.top_talkers[0].message_count, 32);
        assert!(
            stats.top_talkers.iter().all(|talker| talker.error == 0),
            "no eviction below capacity means no overestimate: {:?}",
            stats.top_talkers
        );
    }

    /// Snapshots must report the bucket width, because `coarsen_rate_data`
    /// changes it mid-session and a consumer converting counts to a per-minute
    /// rate cannot otherwise know the divisor.
    #[test]
    fn snapshots_report_the_rate_bucket_width() {
        let mut agg = StatisticsAggregator::with_config(10, 10, 10);
        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        agg.record_message(&chat_at("u1", "User", "msg", base));
        assert_eq!(agg.current_stats().bucket_duration_secs, 10);

        // Push past the point budget so the resolution halves.
        let buckets = agg.max_rate_points as i64 + 10;
        for i in 0..buckets {
            agg.record_message(&chat_at(
                "u1",
                "User",
                "msg",
                base + chrono::Duration::seconds(i * 10),
            ));
        }

        let stats = agg.finalize(base + chrono::Duration::seconds(buckets * 10));
        assert!(
            stats.bucket_duration_secs > 10,
            "coarsening must be visible to consumers, got {}",
            stats.bucket_duration_secs
        );
    }

    /// A checkpoint must let counting continue, not restart. The published
    /// statistics cannot do this: a HyperLogLog estimate does not yield back its
    /// registers, and a truncated top-N does not yield back its counters.
    #[test]
    fn checkpoint_round_trip_continues_counting() {
        let config = StatisticsConfig::default();
        let mut first = StatisticsAggregator::with_settings(config.clone());
        let base = Utc.with_ymd_and_hms(2024, 1, 1, 12, 0, 0).unwrap();

        for i in 0..40 {
            let user = format!("user-{}", i % 4);
            first.record_message(&chat_at(
                &user,
                "User",
                "hello world",
                base + chrono::Duration::seconds(i),
            ));
        }
        let before = first.current_stats();

        let resumed = StatisticsAggregator::from_state(first.export_state(), config)
            .expect("a checkpoint at the current version must load");
        let after = resumed.current_stats();

        assert_eq!(after.total_count, before.total_count);
        assert_eq!(after.unique_talkers, before.unique_talkers);
        assert_eq!(after.start_time, before.start_time);
        assert_eq!(after.bucket_duration_secs, before.bucket_duration_secs);
        assert_eq!(
            after.top_talkers.len(),
            before.top_talkers.len(),
            "talker counters must survive the round trip"
        );
        assert_eq!(
            after.word_frequency.first().map(|entry| entry.count),
            before.word_frequency.first().map(|entry| entry.count)
        );

        // Continuing must add to the restored totals rather than start over.
        let mut resumed = resumed;
        resumed.record_message(&chat_at(
            "user-9",
            "User",
            "extra",
            base + chrono::Duration::seconds(41),
        ));
        assert_eq!(resumed.current_stats().total_count, before.total_count + 1);
    }

    /// A checkpoint from an unrecognized layout is refused, so bumping
    /// `AggregatorState::VERSION` degrades to a fresh aggregator rather than
    /// loading fields that no longer mean what they did.
    #[test]
    fn checkpoint_from_another_version_is_refused() {
        let mut agg = StatisticsAggregator::new();
        agg.record_message(&chat_at("u1", "User", "hi", Utc::now()));

        let mut state = agg.export_state();
        state.version = AggregatorState::VERSION + 1;

        assert!(
            StatisticsAggregator::from_state(state, StatisticsConfig::default()).is_none(),
            "a future layout version must not be loaded"
        );
    }

    /// A checkpoint taken with a larger capacity must load under a smaller one,
    /// keeping the highest counters rather than overflowing the new bound.
    #[test]
    fn checkpoint_shrinks_to_a_smaller_capacity() {
        let mut agg = StatisticsAggregator::with_settings(StatisticsConfig {
            talker_capacity: 512,
            ..StatisticsConfig::default()
        });
        let now = Utc::now();
        for i in 0..200 {
            let user = format!("user-{i}");
            for _ in 0..=i {
                agg.record_message(&chat_at(&user, "User", "msg", now));
            }
        }

        let resumed = StatisticsAggregator::from_state(
            agg.export_state(),
            StatisticsConfig {
                top_talkers: 10,
                talker_capacity: 32,
                ..StatisticsConfig::default()
            },
        )
        .expect("checkpoint loads");

        assert!(resumed.talker_hh.counters.len() <= 32);
        let stats = resumed.current_stats();
        assert_eq!(
            stats.top_talkers[0].user_id, "user-199",
            "the busiest talkers must be the ones kept"
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
        // Small capacities keep the eviction path hot, which is what this test
        // bounds; the defaults would rarely evict at this message count.
        let mut agg = StatisticsAggregator::with_settings(StatisticsConfig {
            top_talkers: 10,
            top_words: 50,
            talker_capacity: 80,
            word_capacity: 200,
            ..StatisticsConfig::default()
        });
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
