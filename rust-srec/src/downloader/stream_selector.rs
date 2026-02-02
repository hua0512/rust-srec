//! Stream selector for choosing the best stream from available options.
//!
//! This module provides filtering and sorting logic to select the optimal
//! stream based on user preferences (quality, format, CDN, bitrate, etc.).

use platforms_parser::media::{StreamFormat, StreamInfo, formats::MediaFormat};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use tracing::debug;

/// Configuration for stream selection preferences.
///
/// Quality names vary by platform:
/// - Chinese platforms (Huya, Douyu, Bilibili): "原画", "蓝光", "超清", "高清", "流畅"
/// - Western platforms (Twitch, YouTube): "1080p60", "1080p", "720p60", "720p", "480p", "360p"
/// - Some platforms use descriptive names: "source", "high", "medium", "low"
///
/// Configure platform-specific quality preferences at the platform or template level.
/// The matching uses substring matching, so "1080" will match "1080p", "1080p60", etc.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StreamSelectionConfig {
    /// Preferred stream formats in order of preference.
    /// None or empty means accept any format.
    pub preferred_formats: Option<Vec<StreamFormat>>,
    /// Preferred media formats in order of preference.
    /// Empty means accept any format.
    pub preferred_media_formats: Vec<MediaFormat>,
    /// Preferred quality levels in order of preference.
    /// Uses substring matching - "1080" matches "1080p", "1080p60", etc.
    /// Platform-specific examples:
    /// - Chinese: ["原画", "蓝光", "超清", "高清"]
    /// - Western: ["source", "1080p", "720p", "480p"]
    ///   Empty means accept any quality (falls back to bitrate/priority sorting).
    pub preferred_qualities: Vec<String>,
    /// Preferred CDN providers in order of preference.
    /// Empty means accept any CDN.
    pub preferred_cdns: Vec<String>,
    /// Blacklisted CDN providers that should be excluded.
    /// Streams from these CDNs will be filtered out entirely.
    /// Uses substring matching (case-insensitive).
    pub blacklisted_cdns: Vec<String>,
    /// Minimum bitrate in bits per second (0 = no minimum).
    pub min_bitrate: u64,
    /// Maximum bitrate in bits per second (0 = no maximum).
    pub max_bitrate: u64,
}

impl StreamSelectionConfig {
    /// Merge another config into this one, with the other config taking precedence.
    ///
    /// Non-empty/non-zero values from `other` override values from `self`.
    /// This supports the layered config hierarchy (global → platform → template → streamer).
    #[must_use]
    pub fn merge(&self, other: &Self) -> Self {
        Self {
            // Use other's formats if specified AND non-empty, otherwise keep self's
            preferred_formats: match &other.preferred_formats {
                Some(formats) if !formats.is_empty() => other.preferred_formats.clone(),
                _ => self.preferred_formats.clone(),
            },
            preferred_media_formats: if other.preferred_media_formats.is_empty() {
                self.preferred_media_formats.clone()
            } else {
                other.preferred_media_formats.clone()
            },
            preferred_qualities: if other.preferred_qualities.is_empty() {
                self.preferred_qualities.clone()
            } else {
                other.preferred_qualities.clone()
            },
            preferred_cdns: if other.preferred_cdns.is_empty() {
                self.preferred_cdns.clone()
            } else {
                other.preferred_cdns.clone()
            },
            // Blacklists are combined (union) rather than replaced
            // This ensures child configs can add to parent blacklists
            blacklisted_cdns: {
                let mut combined = self.blacklisted_cdns.clone();
                for cdn in &other.blacklisted_cdns {
                    if !combined.contains(cdn) {
                        combined.push(cdn.clone());
                    }
                }
                combined
            },
            // Use other's bitrate limits if non-zero
            min_bitrate: if other.min_bitrate > 0 {
                other.min_bitrate
            } else {
                self.min_bitrate
            },
            max_bitrate: if other.max_bitrate > 0 {
                other.max_bitrate
            } else {
                self.max_bitrate
            },
        }
    }
}

/// Stream selector for choosing the best stream.
pub struct StreamSelector {
    config: StreamSelectionConfig,
}

impl StreamSelector {
    /// Create a new stream selector with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(StreamSelectionConfig::default())
    }

    /// Create a new stream selector with custom configuration.
    #[must_use]
    pub fn with_config(config: StreamSelectionConfig) -> Self {
        Self { config }
    }

    /// Select the best stream from the available streams.
    ///
    /// Returns the best matching stream, or None if no streams match the criteria.
    #[must_use]
    pub fn select_best<'a>(&self, streams: &'a [StreamInfo]) -> Option<&'a StreamInfo> {
        self.sort_candidates(streams).first().copied()
    }

    /// Sort candidates by preference.
    #[must_use]
    pub fn sort_candidates<'a>(&self, streams: &'a [StreamInfo]) -> Vec<&'a StreamInfo> {
        if streams.is_empty() {
            return Vec::new();
        }

        // First, filter out blacklisted CDNs (hard exclude, always applied)
        let non_blacklisted: Vec<&StreamInfo> = streams
            .iter()
            .filter(|s| !self.is_cdn_blacklisted(s))
            .collect();

        // If all streams are blacklisted, return empty
        if non_blacklisted.is_empty() {
            return Vec::new();
        }

        // Filter streams based on criteria (bitrate, etc.)
        let filtered: Vec<&StreamInfo> = non_blacklisted
            .iter()
            .filter(|s| self.matches_criteria(s))
            .copied()
            .collect();

        // If no streams match criteria, fall back to non-blacklisted streams
        let candidates = if filtered.is_empty() {
            debug!(
                available = non_blacklisted.len(),
                min_bitrate = self.config.min_bitrate,
                max_bitrate = self.config.max_bitrate,
                "stream selection fallback (no candidates matched criteria)"
            );
            non_blacklisted
        } else {
            filtered
        };

        // Sort by preference and return the best one
        let mut sorted = candidates;
        sorted.sort_by(|a, b| self.compare_streams(a, b));

        sorted
    }

    /// Check if a stream matches the selection criteria.
    #[inline]
    fn matches_criteria(&self, stream: &StreamInfo) -> bool {
        // Check bitrate constraints
        if self.config.min_bitrate > 0 && stream.bitrate < self.config.min_bitrate {
            return false;
        }
        if self.config.max_bitrate > 0 && stream.bitrate > self.config.max_bitrate {
            return false;
        }

        true
    }

    /// Compare two streams for sorting (returns Ordering).
    /// Lower is better (will be sorted first).
    fn compare_streams(&self, a: &StreamInfo, b: &StreamInfo) -> std::cmp::Ordering {
        // Priority Order:
        // 1. Quality Preference
        // 2. CDN Preference
        // 3. Format Preference
        // 4. Media Format Preference
        // 5. Priority Field (lower value = higher priority)
        // 6. Bitrate (higher value = better)

        self.quality_score(a)
            .cmp(&self.quality_score(b))
            .then_with(|| self.cdn_score(a).cmp(&self.cdn_score(b)))
            .then_with(|| self.format_score(a).cmp(&self.format_score(b)))
            .then_with(|| self.media_format_score(a).cmp(&self.media_format_score(b)))
            .then_with(|| a.priority.cmp(&b.priority))
            .then_with(|| b.bitrate.cmp(&a.bitrate))
    }

    /// Get the format preference score (lower is better).
    fn format_score(&self, stream: &StreamInfo) -> usize {
        match &self.config.preferred_formats {
            Some(formats) if !formats.is_empty() => formats
                .iter()
                .position(|f| f == &stream.stream_format)
                .unwrap_or(usize::MAX),
            // No preference or empty list -> equal score
            _ => 0,
        }
    }

    /// Get the media format preference score (lower is better).
    fn media_format_score(&self, stream: &StreamInfo) -> usize {
        if self.config.preferred_media_formats.is_empty() {
            return 0;
        }

        self.config
            .preferred_media_formats
            .iter()
            .position(|f| f == &stream.media_format)
            .unwrap_or(usize::MAX)
    }

    /// Get the quality preference score (lower is better).
    ///
    /// Uses case-insensitive substring matching to handle platform variations.
    fn quality_score(&self, stream: &StreamInfo) -> usize {
        if self.config.preferred_qualities.is_empty() {
            return 0;
        }

        let stream_quality_lower = stream.quality.to_lowercase();

        self.config
            .preferred_qualities
            .iter()
            .position(|q| {
                let pref_lower = q.to_lowercase();
                // Check both directions for flexibility
                stream_quality_lower.contains(&pref_lower)
                    || pref_lower.contains(&stream_quality_lower)
            })
            .unwrap_or(usize::MAX)
    }

    /// Extract CDN identifier from stream extras or fall back to URL.
    /// Returns a reference when possible to avoid allocation.
    #[inline]
    fn get_cdn_source<'a>(&self, stream: &'a StreamInfo) -> Cow<'a, str> {
        stream
            .extras
            .as_ref()
            .and_then(|e| e.get("cdn"))
            .and_then(|v| v.as_str())
            .map(Cow::Borrowed)
            .unwrap_or_else(|| Cow::Borrowed(&stream.url))
    }

    /// Get the CDN preference score (lower is better).
    fn cdn_score(&self, stream: &StreamInfo) -> usize {
        if self.config.preferred_cdns.is_empty() {
            // If no CDN preference, return 0 (neutral) so it doesn't affect sorting
            // unless we want to prioritize based on something else?
            // Actually, existing implementation returned 0 which is correct.
            return 0;
        }

        let cdn_source = self.get_cdn_source(stream);
        let cdn_lower = cdn_source.to_lowercase();
        self.config
            .preferred_cdns
            .iter()
            .position(|c| cdn_lower.contains(&c.to_lowercase()))
            .unwrap_or(usize::MAX)
    }

    /// Check if a stream's CDN is blacklisted.
    /// Uses case-insensitive substring matching.
    /// Falls back to URL matching if CDN info is not available in extras.
    fn is_cdn_blacklisted(&self, stream: &StreamInfo) -> bool {
        if self.config.blacklisted_cdns.is_empty() {
            return false;
        }

        let cdn_source = self.get_cdn_source(stream);
        let cdn_lower = cdn_source.to_lowercase();
        self.config
            .blacklisted_cdns
            .iter()
            .any(|blacklisted| cdn_lower.contains(&blacklisted.to_lowercase()))
    }
}

impl Default for StreamSelector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_stream(
        url: &str,
        format: StreamFormat,
        quality: &str,
        bitrate: u64,
        priority: u32,
    ) -> StreamInfo {
        StreamInfo {
            url: url.to_string(),
            stream_format: format,
            media_format: MediaFormat::Flv,
            quality: quality.to_string(),
            bitrate,
            priority,
            extras: None,
            codec: "h264".to_string(),
            fps: 30.0,
            is_headers_needed: false,
            is_audio_only: false,
        }
    }

    // ========== StreamSelectionConfig::merge tests ==========

    #[test]
    fn test_merge_empty_other_keeps_self() {
        let base = StreamSelectionConfig {
            preferred_formats: Some(vec![StreamFormat::Flv]),
            preferred_qualities: vec!["1080p".to_string()],
            preferred_cdns: vec!["cdn1".to_string()],
            min_bitrate: 1000,
            max_bitrate: 5000,
            ..Default::default()
        };
        let other = StreamSelectionConfig {
            preferred_formats: None,
            preferred_qualities: vec![],
            preferred_cdns: vec![],
            min_bitrate: 0,
            max_bitrate: 0,
            ..Default::default()
        };

        let merged = base.merge(&other);

        assert_eq!(merged.preferred_formats, Some(vec![StreamFormat::Flv]));
        assert_eq!(merged.preferred_qualities, vec!["1080p".to_string()]);
        assert_eq!(merged.preferred_cdns, vec!["cdn1".to_string()]);
        assert_eq!(merged.min_bitrate, 1000);
        assert_eq!(merged.max_bitrate, 5000);
    }

    #[test]
    fn test_merge_other_overrides_self() {
        let base = StreamSelectionConfig {
            preferred_formats: Some(vec![StreamFormat::Flv]),
            preferred_qualities: vec!["1080p".to_string()],
            min_bitrate: 1000,
            ..Default::default()
        };
        let other = StreamSelectionConfig {
            preferred_formats: Some(vec![StreamFormat::Hls]),
            preferred_qualities: vec!["720p".to_string()],
            min_bitrate: 2000,
            ..Default::default()
        };

        let merged = base.merge(&other);

        assert_eq!(merged.preferred_formats, Some(vec![StreamFormat::Hls]));
        assert_eq!(merged.preferred_qualities, vec!["720p".to_string()]);
        assert_eq!(merged.min_bitrate, 2000);
    }

    #[test]
    fn test_merge_partial_override() {
        let base = StreamSelectionConfig {
            preferred_formats: Some(vec![StreamFormat::Flv]),
            preferred_qualities: vec!["1080p".to_string()],
            preferred_cdns: vec!["cdn1".to_string()],
            min_bitrate: 1000,
            max_bitrate: 5000,
            ..Default::default()
        };
        let other = StreamSelectionConfig {
            preferred_formats: Some(vec![StreamFormat::Hls]), // Override
            preferred_qualities: vec![],                      // Keep base
            preferred_cdns: vec![],                           // Keep base
            min_bitrate: 0,                                   // Keep base
            max_bitrate: 8000,                                // Override
            ..Default::default()
        };

        let merged = base.merge(&other);

        assert_eq!(merged.preferred_formats, Some(vec![StreamFormat::Hls])); // Overridden
        assert_eq!(merged.preferred_qualities, vec!["1080p".to_string()]); // Kept
        assert_eq!(merged.preferred_cdns, vec!["cdn1".to_string()]); // Kept
        assert_eq!(merged.min_bitrate, 1000); // Kept
        assert_eq!(merged.max_bitrate, 8000); // Overridden
    }

    #[test]
    fn test_merge_empty_vec_preserves_base() {
        // This test verifies that Some(vec![]) (from JSON "preferred_formats": [])
        // is treated the same as None and preserves the base config
        let base = StreamSelectionConfig {
            preferred_formats: Some(vec![StreamFormat::Flv, StreamFormat::Hls]),
            preferred_qualities: vec!["1080p".to_string(), "720p".to_string()],
            ..Default::default()
        };
        let other = StreamSelectionConfig {
            // Simulates deserializing {"preferred_formats": []} from JSON
            preferred_formats: Some(vec![]),
            preferred_qualities: vec![],
            ..Default::default()
        };

        let merged = base.merge(&other);

        // Empty array should NOT override - base config should be preserved
        assert_eq!(
            merged.preferred_formats,
            Some(vec![StreamFormat::Flv, StreamFormat::Hls])
        );
        assert_eq!(
            merged.preferred_qualities,
            vec!["1080p".to_string(), "720p".to_string()]
        );
    }

    // ========== StreamSelector tests ==========

    #[test]
    fn test_select_best_empty() {
        let selector = StreamSelector::new();
        assert!(selector.select_best(&[]).is_none());
    }

    #[test]
    fn test_select_best_single() {
        let selector = StreamSelector::new();
        let streams = vec![create_test_stream(
            "http://example.com/stream.flv",
            StreamFormat::Flv,
            "1080p",
            5000000,
            1,
        )];
        let best = selector.select_best(&streams);
        assert!(best.is_some());
        assert_eq!(best.unwrap().url, "http://example.com/stream.flv");
    }

    #[test]
    fn test_select_best_by_format() {
        let config = StreamSelectionConfig {
            preferred_formats: Some(vec![StreamFormat::Flv]),
            ..Default::default()
        };
        let selector = StreamSelector::with_config(config);

        let streams = vec![
            create_test_stream(
                "http://example.com/stream.m3u8",
                StreamFormat::Hls,
                "1080p",
                5000000,
                1,
            ),
            create_test_stream(
                "http://example.com/stream.flv",
                StreamFormat::Flv,
                "1080p",
                5000000,
                1,
            ),
        ];

        let best = selector.select_best(&streams);
        assert!(best.is_some());
        assert_eq!(best.unwrap().stream_format, StreamFormat::Flv);
    }

    #[test]
    fn test_select_best_by_quality() {
        let config = StreamSelectionConfig {
            preferred_qualities: vec!["原画".to_string(), "1080p".to_string()],
            ..Default::default()
        };
        let selector = StreamSelector::with_config(config);

        let streams = vec![
            create_test_stream(
                "http://example.com/720p.flv",
                StreamFormat::Flv,
                "720p",
                3000000,
                1,
            ),
            create_test_stream(
                "http://example.com/1080p.flv",
                StreamFormat::Flv,
                "1080p",
                5000000,
                1,
            ),
        ];

        let best = selector.select_best(&streams);
        assert!(best.is_some());
        assert!(best.unwrap().quality.contains("1080p"));
    }

    #[test]
    fn test_select_best_by_bitrate() {
        let config = StreamSelectionConfig {
            min_bitrate: 4000000,
            ..Default::default()
        };
        let selector = StreamSelector::with_config(config);

        let streams = vec![
            create_test_stream(
                "http://example.com/low.flv",
                StreamFormat::Flv,
                "720p",
                3000000,
                1,
            ),
            create_test_stream(
                "http://example.com/high.flv",
                StreamFormat::Flv,
                "1080p",
                5000000,
                1,
            ),
        ];

        let best = selector.select_best(&streams);
        assert!(best.is_some());
        assert_eq!(best.unwrap().bitrate, 5000000);
    }

    #[test]
    fn test_select_best_by_priority() {
        let selector = StreamSelector::new();

        let streams = vec![
            create_test_stream(
                "http://example.com/low.flv",
                StreamFormat::Flv,
                "1080p",
                5000000,
                2,
            ),
            create_test_stream(
                "http://example.com/high.flv",
                StreamFormat::Flv,
                "1080p",
                5000000,
                1,
            ),
        ];

        let best = selector.select_best(&streams);
        assert!(best.is_some());
        assert_eq!(best.unwrap().priority, 1);
    }

    #[test]
    fn test_select_best_quality_case_insensitive() {
        // Test that quality matching is case-insensitive
        let config = StreamSelectionConfig {
            preferred_qualities: vec!["SOURCE".to_string(), "1080P".to_string()],
            ..Default::default()
        };
        let selector = StreamSelector::with_config(config);

        let streams = vec![
            create_test_stream(
                "http://example.com/720p.flv",
                StreamFormat::Flv,
                "720p",
                3000000,
                1,
            ),
            create_test_stream(
                "http://example.com/1080p.flv",
                StreamFormat::Flv,
                "1080p",
                5000000,
                1,
            ),
        ];

        let best = selector.select_best(&streams);
        assert!(best.is_some());
        assert!(best.unwrap().quality.contains("1080p"));
    }

    #[test]
    fn test_select_best_quality_chinese_names() {
        // Test Chinese quality names (common on Huya, Douyu, Bilibili)
        let config = StreamSelectionConfig {
            preferred_qualities: vec!["原画".to_string(), "蓝光".to_string()],
            ..Default::default()
        };
        let selector = StreamSelector::with_config(config);

        let streams = vec![
            create_test_stream(
                "http://example.com/hd.flv",
                StreamFormat::Flv,
                "蓝光4M",
                4000000,
                1,
            ),
            create_test_stream(
                "http://example.com/source.flv",
                StreamFormat::Flv,
                "原画",
                8000000,
                1,
            ),
        ];

        let best = selector.select_best(&streams);
        assert!(best.is_some());
        assert!(best.unwrap().quality.contains("原画"));
    }

    #[test]
    fn test_select_best_fallback_to_bitrate() {
        // When no quality preferences match, should fall back to highest bitrate
        let config = StreamSelectionConfig {
            preferred_qualities: vec!["4K".to_string()], // Won't match any stream
            ..Default::default()
        };
        let selector = StreamSelector::with_config(config);

        let streams = vec![
            create_test_stream(
                "http://example.com/low.flv",
                StreamFormat::Flv,
                "720p",
                3000000,
                1,
            ),
            create_test_stream(
                "http://example.com/high.flv",
                StreamFormat::Flv,
                "1080p",
                5000000,
                1,
            ),
        ];

        let best = selector.select_best(&streams);
        assert!(best.is_some());
        // Should pick higher bitrate when quality doesn't match
        assert_eq!(best.unwrap().bitrate, 5000000);
    }

    #[test]
    fn test_select_best_cdn_priority_over_format() {
        // Test that preferred CDN is selected even if it's not the preferred format
        // Default config should have no preference for format, but we set one here to test priority
        let config = StreamSelectionConfig {
            preferred_cdns: vec!["akm".to_string()],
            preferred_formats: Some(vec![StreamFormat::Flv]), // FLV preferred
            ..Default::default()
        };
        let selector = StreamSelector::with_config(config);

        let mut stream_hls_preferred_cdn = create_test_stream(
            "http://example.com/hls.m3u8",
            StreamFormat::Hls,
            "1080p",
            10000,
            1,
        );
        let mut extras = serde_json::Map::new();
        extras.insert(
            "cdn".to_string(),
            serde_json::Value::String("akm".to_string()),
        );
        stream_hls_preferred_cdn.extras = Some(serde_json::Value::Object(extras));

        let mut stream_flv_other_cdn = create_test_stream(
            "http://example.com/flv.flv",
            StreamFormat::Flv,
            "1080p",
            10000,
            1,
        );
        let mut extras_flv = serde_json::Map::new();
        extras_flv.insert(
            "cdn".to_string(),
            serde_json::Value::String("other".to_string()),
        );
        stream_flv_other_cdn.extras = Some(serde_json::Value::Object(extras_flv));

        let streams = vec![stream_hls_preferred_cdn, stream_flv_other_cdn];

        let best = selector.select_best(&streams);
        assert!(best.is_some());

        // With Quality > CDN > Format priority, CDN should win even if Format prefers FLV
        assert_eq!(best.unwrap().stream_format, StreamFormat::Hls);
        assert_eq!(best.unwrap().url, "http://example.com/hls.m3u8");
    }

    #[test]
    fn test_blacklisted_cdn_excluded() {
        let config = StreamSelectionConfig {
            blacklisted_cdns: vec!["badcdn".to_string()],
            ..Default::default()
        };
        let selector = StreamSelector::with_config(config);

        let mut stream_good = create_test_stream(
            "http://example.com/good.flv",
            StreamFormat::Flv,
            "1080p",
            5000000,
            1,
        );
        let mut extras_good = serde_json::Map::new();
        extras_good.insert(
            "cdn".to_string(),
            serde_json::Value::String("goodcdn".to_string()),
        );
        stream_good.extras = Some(serde_json::Value::Object(extras_good));

        let mut stream_bad = create_test_stream(
            "http://example.com/bad.flv",
            StreamFormat::Flv,
            "1080p",
            5000000,
            1,
        );
        let mut extras_bad = serde_json::Map::new();
        extras_bad.insert(
            "cdn".to_string(),
            serde_json::Value::String("badcdn-server1".to_string()),
        );
        stream_bad.extras = Some(serde_json::Value::Object(extras_bad));

        let streams = vec![stream_bad, stream_good];
        let best = selector.select_best(&streams);

        assert!(best.is_some());
        assert_eq!(best.unwrap().url, "http://example.com/good.flv");
    }

    #[test]
    fn test_blacklist_case_insensitive() {
        let config = StreamSelectionConfig {
            blacklisted_cdns: vec!["BADCDN".to_string()],
            ..Default::default()
        };
        let selector = StreamSelector::with_config(config);

        let mut stream = create_test_stream(
            "http://example.com/stream.flv",
            StreamFormat::Flv,
            "1080p",
            5000000,
            1,
        );
        let mut extras = serde_json::Map::new();
        extras.insert(
            "cdn".to_string(),
            serde_json::Value::String("badcdn".to_string()),
        );
        stream.extras = Some(serde_json::Value::Object(extras));

        let streams = vec![stream];
        let result = selector.select_best(&streams);

        // Stream should be blacklisted (case insensitive match)
        assert!(result.is_none());
    }

    #[test]
    fn test_merge_blacklists_combined() {
        let base = StreamSelectionConfig {
            blacklisted_cdns: vec!["cdn1".to_string(), "cdn2".to_string()],
            ..Default::default()
        };
        let other = StreamSelectionConfig {
            blacklisted_cdns: vec!["cdn2".to_string(), "cdn3".to_string()],
            ..Default::default()
        };

        let merged = base.merge(&other);

        // Should contain all unique CDNs from both
        assert!(merged.blacklisted_cdns.contains(&"cdn1".to_string()));
        assert!(merged.blacklisted_cdns.contains(&"cdn2".to_string()));
        assert!(merged.blacklisted_cdns.contains(&"cdn3".to_string()));
        assert_eq!(merged.blacklisted_cdns.len(), 3);
    }

    #[test]
    fn test_stream_without_cdn_not_blacklisted() {
        let config = StreamSelectionConfig {
            blacklisted_cdns: vec!["badcdn".to_string()],
            ..Default::default()
        };
        let selector = StreamSelector::with_config(config);

        // Stream without CDN info
        let stream = create_test_stream(
            "http://example.com/stream.flv",
            StreamFormat::Flv,
            "1080p",
            5000000,
            1,
        );

        let streams = vec![stream];
        let result = selector.select_best(&streams);

        // Stream without CDN info should NOT be blacklisted
        assert!(result.is_some());
    }

    #[test]
    fn test_blacklist_with_preferences() {
        // Preferred CDN should still be excluded if also blacklisted
        let config = StreamSelectionConfig {
            preferred_cdns: vec!["goodcdn".to_string(), "badcdn".to_string()],
            blacklisted_cdns: vec!["badcdn".to_string()],
            ..Default::default()
        };
        let selector = StreamSelector::with_config(config);

        let mut stream_good = create_test_stream(
            "http://example.com/good.flv",
            StreamFormat::Flv,
            "1080p",
            5000000,
            2, // Lower priority
        );
        let mut extras_good = serde_json::Map::new();
        extras_good.insert(
            "cdn".to_string(),
            serde_json::Value::String("goodcdn".to_string()),
        );
        stream_good.extras = Some(serde_json::Value::Object(extras_good));

        let mut stream_bad = create_test_stream(
            "http://example.com/bad.flv",
            StreamFormat::Flv,
            "1080p",
            5000000,
            1, // Higher priority
        );
        let mut extras_bad = serde_json::Map::new();
        extras_bad.insert(
            "cdn".to_string(),
            serde_json::Value::String("badcdn".to_string()),
        );
        stream_bad.extras = Some(serde_json::Value::Object(extras_bad));

        let streams = vec![stream_bad, stream_good];
        let best = selector.select_best(&streams);

        // Even though badcdn has higher priority and is preferred,
        // it should be excluded because it's blacklisted
        assert!(best.is_some());
        assert_eq!(best.unwrap().url, "http://example.com/good.flv");
    }

    #[test]
    fn test_blacklist_matches_url_when_no_cdn_extra() {
        let config = StreamSelectionConfig {
            blacklisted_cdns: vec!["akamaized".to_string()],
            ..Default::default()
        };
        let selector = StreamSelector::with_config(config);

        // Stream with akamaized in URL but no cdn in extras
        let stream = create_test_stream(
            "https://video.akamaized.net/stream.m3u8",
            StreamFormat::Hls,
            "1080p",
            5000000,
            1,
        );

        let streams = vec![stream];
        let result = selector.select_best(&streams);

        // Should be blacklisted based on URL
        assert!(result.is_none());
    }

    #[test]
    fn test_preferred_cdn_matches_url_when_no_cdn_extra() {
        let config = StreamSelectionConfig {
            preferred_cdns: vec!["cloudfront".to_string()],
            ..Default::default()
        };
        let selector = StreamSelector::with_config(config);

        let stream_cloudfront = create_test_stream(
            "https://d1234.cloudfront.net/stream.m3u8",
            StreamFormat::Hls,
            "1080p",
            5000000,
            2,
        );

        let stream_other = create_test_stream(
            "https://other-cdn.example.com/stream.m3u8",
            StreamFormat::Hls,
            "1080p",
            5000000,
            1,
        );

        let streams = vec![stream_other, stream_cloudfront];
        let best = selector.select_best(&streams);

        // Should prefer cloudfront based on URL matching
        assert!(best.is_some());
        assert!(best.unwrap().url.contains("cloudfront"));
    }

    #[test]
    fn test_cdn_extra_takes_precedence_over_url() {
        // When extras["cdn"] exists, it should be used instead of URL
        let config = StreamSelectionConfig {
            blacklisted_cdns: vec!["badcdn".to_string()],
            ..Default::default()
        };
        let selector = StreamSelector::with_config(config);

        // Stream with "badcdn" in URL but different cdn in extras
        let mut stream = create_test_stream(
            "https://badcdn.example.com/stream.m3u8",
            StreamFormat::Hls,
            "1080p",
            5000000,
            1,
        );
        let mut extras = serde_json::Map::new();
        extras.insert(
            "cdn".to_string(),
            serde_json::Value::String("goodcdn".to_string()),
        );
        stream.extras = Some(serde_json::Value::Object(extras));

        let streams = vec![stream];
        let result = selector.select_best(&streams);

        // Should NOT be blacklisted because extras["cdn"] = "goodcdn" takes precedence
        assert!(result.is_some());
    }
}
