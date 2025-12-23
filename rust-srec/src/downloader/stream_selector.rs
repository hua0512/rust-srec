//! Stream selector for choosing the best stream from available options.
//!
//! This module provides filtering and sorting logic to select the optimal
//! stream based on user preferences (quality, format, CDN, bitrate, etc.).

use platforms_parser::media::{StreamFormat, StreamInfo, formats::MediaFormat};
use serde::{Deserialize, Serialize};
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
    pub fn merge(&self, other: &Self) -> Self {
        Self {
            // Use other's formats if specified (Some), otherwise keep self's
            preferred_formats: if other.preferred_formats.is_some() {
                other.preferred_formats.clone()
            } else {
                self.preferred_formats.clone()
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
    pub fn new() -> Self {
        Self::with_config(StreamSelectionConfig::default())
    }

    /// Create a new stream selector with custom configuration.
    pub fn with_config(config: StreamSelectionConfig) -> Self {
        Self { config }
    }

    /// Select the best stream from the available streams.
    ///
    /// Returns the best matching stream, or None if no streams match the criteria.
    pub fn select_best<'a>(&self, streams: &'a [StreamInfo]) -> Option<&'a StreamInfo> {
        self.sort_candidates(streams).first().copied()
    }

    /// Sort candidates by preference.
    pub fn sort_candidates<'a>(&self, streams: &'a [StreamInfo]) -> Vec<&'a StreamInfo> {
        if streams.is_empty() {
            return Vec::new();
        }

        // Filter streams based on criteria
        let filtered: Vec<&StreamInfo> = streams
            .iter()
            .filter(|s| self.matches_criteria(s))
            .collect();

        // If no streams match criteria, fall back to all streams
        let candidates = if filtered.is_empty() {
            debug!("No streams match selection criteria, using all streams");
            streams.iter().collect()
        } else {
            filtered
        };

        // Sort by preference and return the best one
        let mut sorted = candidates;
        sorted.sort_by(|a, b| self.compare_streams(a, b));

        sorted
    }

    /// Check if a stream matches the selection criteria.
    fn matches_criteria(&self, stream: &StreamInfo) -> bool {
        // Check bitrate constraints
        if self.config.min_bitrate > 0 && stream.bitrate < self.config.min_bitrate {
            return false;
        }
        if self.config.max_bitrate > 0 && stream.bitrate > self.config.max_bitrate {
            return false;
        }

        // Check format constraints (if specified)
        if let Some(formats) = &self.config.preferred_formats
            && !formats.is_empty()
            && !formats.contains(&stream.stream_format)
        {
            return false;
        }

        // Check media format constraints (if specified)
        if !self.config.preferred_media_formats.is_empty()
            && !self
                .config
                .preferred_media_formats
                .contains(&stream.media_format)
        {
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
        // 4. Priority Field (lower value = higher priority)
        // 5. Bitrate (higher value = better)

        // 1. Compare by quality preference
        let quality_score_a = self.quality_score(a);
        let quality_score_b = self.quality_score(b);
        if quality_score_a != quality_score_b {
            return quality_score_a.cmp(&quality_score_b);
        }

        // 2. Compare by CDN preference
        let cdn_score_a = self.cdn_score(a);
        let cdn_score_b = self.cdn_score(b);
        if cdn_score_a != cdn_score_b {
            return cdn_score_a.cmp(&cdn_score_b);
        }

        // 3. Compare by format preference
        let format_score_a = self.format_score(a);
        let format_score_b = self.format_score(b);
        if format_score_a != format_score_b {
            return format_score_a.cmp(&format_score_b);
        }

        // 4. Compare by priority (lower priority value = higher priority)
        if a.priority != b.priority {
            return a.priority.cmp(&b.priority);
        }

        // 5. Compare by bitrate (higher is better)
        b.bitrate.cmp(&a.bitrate)
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

    /// Get the CDN preference score (lower is better).
    fn cdn_score(&self, stream: &StreamInfo) -> usize {
        if self.config.preferred_cdns.is_empty() {
            // If no CDN preference, return 0 (neutral) so it doesn't affect sorting
            // unless we want to prioritize based on something else?
            // Actually, existing implementation returned 0 which is correct.
            return 0;
        }

        // Extract CDN from extras
        let cdn = stream
            .extras
            .as_ref()
            .and_then(|e| e.get("cdn"))
            .and_then(|v| v.as_str());

        match cdn {
            Some(cdn_name) => self
                .config
                .preferred_cdns
                .iter()
                .position(|c| cdn_name.contains(c))
                .unwrap_or(usize::MAX),
            None => usize::MAX,
        }
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
}
