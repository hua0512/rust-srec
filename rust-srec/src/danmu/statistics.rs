//! Danmu statistics calculation.
//!
//! Provides statistics aggregation for danmu messages during a session.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Statistics for a danmu collection session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DanmuStatistics {
    /// Total number of danmu messages received
    pub total_count: u64,
    /// Number of chat messages
    pub chat_count: u64,
    /// Number of gift messages
    pub gift_count: u64,
    /// Top talkers (user_id -> message count)
    pub top_talkers: Vec<TopTalker>,
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

/// A rate timeseries data point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateDataPoint {
    pub timestamp: DateTime<Utc>,
    pub count: u64,
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
    /// User message counts (user_id -> (username, count))
    user_counts: HashMap<String, (String, u64)>,
    /// Word counts
    word_counts: HashMap<String, u64>,
    /// Rate data points
    rate_data: Vec<RateDataPoint>,
    /// Current rate bucket
    current_bucket: Option<(DateTime<Utc>, u64)>,
    /// Bucket duration in seconds
    bucket_duration_secs: u64,
    /// Session start time
    start_time: Option<DateTime<Utc>>,
    /// Maximum number of top talkers to track
    max_top_talkers: usize,
    /// Maximum number of words to track
    max_words: usize,
    /// Stop words to filter out
    stop_words: std::collections::HashSet<String>,
}

impl StatisticsAggregator {
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
        Self {
            total_count: 0,
            chat_count: 0,
            gift_count: 0,
            user_counts: HashMap::new(),
            word_counts: HashMap::new(),
            rate_data: Vec::new(),
            current_bucket: None,
            bucket_duration_secs,
            start_time: None,
            max_top_talkers,
            max_words,
            stop_words: default_stop_words(),
        }
    }

    /// Record a message.
    pub fn record_message(
        &mut self,
        user_id: &str,
        username: &str,
        content: &str,
        is_gift: bool,
        timestamp: DateTime<Utc>,
    ) {
        // Set start time on first message
        if self.start_time.is_none() {
            self.start_time = Some(timestamp);
        }

        // Update counts
        self.total_count += 1;
        if is_gift {
            self.gift_count += 1;
        } else {
            self.chat_count += 1;
        }

        // Update user counts
        self.user_counts
            .entry(user_id.to_string())
            .and_modify(|(_, count)| *count += 1)
            .or_insert_with(|| (username.to_string(), 1));

        // Update word counts (only for chat messages)
        if !is_gift && !content.is_empty() {
            self.process_words(content);
        }

        // Update rate data
        self.update_rate_bucket(timestamp);
    }

    /// Process words from a message.
    fn process_words(&mut self, content: &str) {
        for word in tokenize(content) {
            let word_lower = word.to_lowercase();

            // Skip stop words and very short words
            if word_lower.len() < 2 || self.stop_words.contains(&word_lower) {
                continue;
            }

            *self.word_counts.entry(word_lower).or_insert(0) += 1;
        }

        // Prune word counts if too large
        if self.word_counts.len() > self.max_words * 2 {
            self.prune_word_counts();
        }
    }

    /// Prune word counts to keep only top words.
    fn prune_word_counts(&mut self) {
        let mut counts: Vec<_> = self.word_counts.drain().collect();
        counts.sort_by(|a, b| b.1.cmp(&a.1));
        counts.truncate(self.max_words);
        self.word_counts = counts.into_iter().collect();
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
                self.rate_data.push(RateDataPoint {
                    timestamp: *start,
                    count: *count,
                });
                self.current_bucket = Some((bucket_start, 1));
            }
            None => {
                self.current_bucket = Some((bucket_start, 1));
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
            self.rate_data.push(RateDataPoint {
                timestamp: start,
                count,
            });
        }

        // Calculate duration
        let duration_secs = self
            .start_time
            .map(|start| (end_time - start).num_seconds().max(0) as u64)
            .unwrap_or(0);

        // Get top talkers
        let mut user_list: Vec<_> = self.user_counts.into_iter().collect();
        user_list.sort_by(|a, b| b.1.1.cmp(&a.1.1));
        let top_talkers: Vec<TopTalker> = user_list
            .into_iter()
            .take(self.max_top_talkers)
            .map(|(user_id, (username, count))| TopTalker {
                user_id,
                username,
                message_count: count,
            })
            .collect();

        // Get word frequency
        let mut word_list: Vec<_> = self.word_counts.into_iter().collect();
        word_list.sort_by(|a, b| b.1.cmp(&a.1));
        let word_frequency: Vec<WordFrequency> = word_list
            .into_iter()
            .take(self.max_words)
            .map(|(word, count)| WordFrequency { word, count })
            .collect();

        DanmuStatistics {
            total_count: self.total_count,
            chat_count: self.chat_count,
            gift_count: self.gift_count,
            top_talkers,
            word_frequency,
            rate_timeseries: self.rate_data,
            start_time: self.start_time,
            end_time: Some(end_time),
            duration_secs,
        }
    }

    /// Get current statistics without finalizing.
    pub fn current_stats(&self) -> DanmuStatistics {
        // Get top talkers
        let mut user_list: Vec<_> = self.user_counts.iter().collect();
        user_list.sort_by(|a, b| b.1.1.cmp(&a.1.1));
        let top_talkers: Vec<TopTalker> = user_list
            .into_iter()
            .take(self.max_top_talkers)
            .map(|(user_id, (username, count))| TopTalker {
                user_id: user_id.clone(),
                username: username.clone(),
                message_count: *count,
            })
            .collect();

        // Get word frequency
        let mut word_list: Vec<_> = self.word_counts.iter().collect();
        word_list.sort_by(|a, b| b.1.cmp(&a.1));
        let word_frequency: Vec<WordFrequency> = word_list
            .into_iter()
            .take(self.max_words)
            .map(|(word, count)| WordFrequency {
                word: word.clone(),
                count: *count,
            })
            .collect();

        let mut rate_data = self.rate_data.clone();
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
            top_talkers,
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

/// Tokenize a message into words.
fn tokenize(content: &str) -> Vec<&str> {
    // Simple tokenization that handles both CJK and Western text
    content
        .split(|c: char| c.is_whitespace() || c.is_ascii_punctuation())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Default stop words for filtering.
fn default_stop_words() -> std::collections::HashSet<String> {
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

    words.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_record_message() {
        let mut agg = StatisticsAggregator::new();
        let now = Utc::now();

        agg.record_message("user1", "User One", "Hello world!", false, now);
        agg.record_message("user2", "User Two", "Hi there!", false, now);
        agg.record_message("user1", "User One", "Another message", false, now);

        let stats = agg.current_stats();
        assert_eq!(stats.total_count, 3);
        assert_eq!(stats.chat_count, 3);
        assert_eq!(stats.gift_count, 0);
    }

    #[test]
    fn test_top_talkers() {
        let mut agg = StatisticsAggregator::with_config(3, 10, 10);
        let now = Utc::now();

        // User1: 5 messages, User2: 3 messages, User3: 1 message
        for _ in 0..5 {
            agg.record_message("user1", "User One", "msg", false, now);
        }
        for _ in 0..3 {
            agg.record_message("user2", "User Two", "msg", false, now);
        }
        agg.record_message("user3", "User Three", "msg", false, now);

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

        agg.record_message("user1", "User", "hello world hello", false, now);
        agg.record_message("user2", "User", "hello rust world", false, now);

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
        agg.record_message("user1", "User", "msg1", false, base);
        agg.record_message(
            "user1",
            "User",
            "msg2",
            false,
            base + chrono::Duration::seconds(5),
        );

        // Messages in second bucket
        agg.record_message(
            "user1",
            "User",
            "msg3",
            false,
            base + chrono::Duration::seconds(15),
        );

        let end_time = base + chrono::Duration::seconds(20);
        let stats = agg.finalize(end_time);

        assert_eq!(stats.rate_timeseries.len(), 2);
        assert_eq!(stats.rate_timeseries[0].count, 2); // First bucket
        assert_eq!(stats.rate_timeseries[1].count, 1); // Second bucket
    }

    #[test]
    fn test_finalize() {
        let mut agg = StatisticsAggregator::new();
        let start = Utc.with_ymd_and_hms(2024, 1, 1, 12, 0, 0).unwrap();
        let end = start + chrono::Duration::minutes(30);

        agg.record_message("user1", "User", "Hello", false, start);

        let stats = agg.finalize(end);

        assert_eq!(stats.start_time, Some(start));
        assert_eq!(stats.end_time, Some(end));
        assert_eq!(stats.duration_secs, 1800); // 30 minutes
    }

    #[test]
    fn test_gift_counting() {
        let mut agg = StatisticsAggregator::new();
        let now = Utc::now();

        agg.record_message("user1", "User", "chat", false, now);
        agg.record_message("user2", "User", "gift", true, now);
        agg.record_message("user3", "User", "gift", true, now);

        let stats = agg.current_stats();
        assert_eq!(stats.total_count, 3);
        assert_eq!(stats.chat_count, 1);
        assert_eq!(stats.gift_count, 2);
    }

    #[test]
    fn test_tokenize() {
        let tokens = tokenize("Hello, world! How are you?");
        assert_eq!(tokens, vec!["Hello", "world", "How", "are", "you"]);

        let cjk_tokens = tokenize("你好 世界");
        assert_eq!(cjk_tokens, vec!["你好", "世界"]);
    }
}
