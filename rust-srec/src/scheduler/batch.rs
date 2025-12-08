//! Batch detection grouping.
//!
//! This module handles grouping streamers by platform for batch detection,
//! identifying which platforms support batch API calls.

use std::collections::HashMap;

use crate::streamer::StreamerMetadata;

/// Platforms that support batch status detection.
const BATCH_SUPPORTED_PLATFORMS: &[&str] = &[
    "twitch", "youtube",
    // Add more platforms as they implement batch detection
];

/// A group of streamers on the same platform for batch detection.
#[derive(Debug, Clone)]
pub struct BatchGroup {
    /// Platform identifier.
    pub platform_id: String,
    /// Streamers in this group.
    pub streamers: Vec<StreamerMetadata>,
    /// Whether this platform supports batch detection.
    pub supports_batch: bool,
}

impl BatchGroup {
    /// Create a new batch group.
    pub fn new(platform_id: String, streamers: Vec<StreamerMetadata>) -> Self {
        let supports_batch = is_batch_supported(&platform_id);
        Self {
            platform_id,
            streamers,
            supports_batch,
        }
    }

    /// Get the number of streamers in this group.
    pub fn len(&self) -> usize {
        self.streamers.len()
    }

    /// Check if the group is empty.
    pub fn is_empty(&self) -> bool {
        self.streamers.is_empty()
    }

    /// Add a streamer to the group.
    pub fn add(&mut self, streamer: StreamerMetadata) {
        self.streamers.push(streamer);
    }
}

/// Check if a platform supports batch detection.
pub fn is_batch_supported(platform_id: &str) -> bool {
    BATCH_SUPPORTED_PLATFORMS
        .iter()
        .any(|&p| p.eq_ignore_ascii_case(platform_id))
}

/// Group streamers by platform.
///
/// Returns a map of platform ID to batch group.
pub fn group_by_platform(streamers: Vec<StreamerMetadata>) -> HashMap<String, BatchGroup> {
    let mut groups: HashMap<String, BatchGroup> = HashMap::new();

    for streamer in streamers {
        let platform_id = streamer.platform_config_id.clone();
        groups
            .entry(platform_id.clone())
            .or_insert_with(|| BatchGroup::new(platform_id, Vec::new()))
            .add(streamer);
    }

    groups
}

/// Separate streamers into batch and individual groups.
///
/// Returns (batch_groups, individual_streamers).
#[allow(dead_code)]
pub fn separate_batch_and_individual(
    streamers: Vec<StreamerMetadata>,
) -> (Vec<BatchGroup>, Vec<StreamerMetadata>) {
    let groups = group_by_platform(streamers);
    let mut batch_groups = Vec::new();
    let mut individual_streamers = Vec::new();

    for (_, group) in groups {
        if group.supports_batch {
            batch_groups.push(group);
        } else {
            individual_streamers.extend(group.streamers);
        }
    }

    (batch_groups, individual_streamers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Priority, StreamerState};

    fn create_test_streamer(id: &str, platform: &str) -> StreamerMetadata {
        StreamerMetadata {
            id: id.to_string(),
            name: format!("Streamer {}", id),
            url: format!("https://{}.tv/{}", platform, id),
            platform_config_id: platform.to_string(),
            template_config_id: None,
            state: StreamerState::NotLive,
            priority: Priority::Normal,
            avatar_url: None,
            consecutive_error_count: 0,
            disabled_until: None,
            last_live_time: None,
        }
    }

    #[test]
    fn test_is_batch_supported() {
        assert!(is_batch_supported("twitch"));
        assert!(is_batch_supported("Twitch")); // Case insensitive
        assert!(is_batch_supported("youtube"));
        assert!(!is_batch_supported("huya"));
        assert!(!is_batch_supported("unknown"));
    }

    #[test]
    fn test_group_by_platform() {
        let streamers = vec![
            create_test_streamer("1", "twitch"),
            create_test_streamer("2", "twitch"),
            create_test_streamer("3", "huya"),
            create_test_streamer("4", "youtube"),
        ];

        let groups = group_by_platform(streamers);

        assert_eq!(groups.len(), 3);
        assert_eq!(groups.get("twitch").unwrap().len(), 2);
        assert_eq!(groups.get("huya").unwrap().len(), 1);
        assert_eq!(groups.get("youtube").unwrap().len(), 1);
    }

    #[test]
    fn test_separate_batch_and_individual() {
        let streamers = vec![
            create_test_streamer("1", "twitch"),
            create_test_streamer("2", "twitch"),
            create_test_streamer("3", "huya"),
            create_test_streamer("4", "youtube"),
            create_test_streamer("5", "huya"),
        ];

        let (batch_groups, individual) = separate_batch_and_individual(streamers);

        // Twitch and YouTube support batch
        assert_eq!(batch_groups.len(), 2);

        // Huya doesn't support batch
        assert_eq!(individual.len(), 2);
        assert!(individual.iter().all(|s| s.platform_config_id == "huya"));
    }

    #[test]
    fn test_batch_group_supports_batch() {
        let twitch_group = BatchGroup::new("twitch".to_string(), vec![]);
        assert!(twitch_group.supports_batch);

        let huya_group = BatchGroup::new("huya".to_string(), vec![]);
        assert!(!huya_group.supports_batch);
    }
}
