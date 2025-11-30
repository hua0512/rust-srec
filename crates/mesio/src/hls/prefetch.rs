// HLS Segment Prefetch Manager: Manages predictive segment prefetching.
//
// This module implements predictive prefetching for HLS segments based on
// playlist analysis. When a segment is successfully downloaded, the prefetch
// manager determines which subsequent segments should be prefetched to reduce
// latency during playback.

use crate::hls::config::PrefetchConfig;
use std::collections::HashSet;
use tracing::debug;

/// Manages segment prefetching for HLS streams.
///
/// The PrefetchManager tracks which segments are pending prefetch and determines
/// which segments should be prefetched based on the current download position
/// and buffer state.
///
/// # Requirements
/// - 2.1: After successful download, initiate prefetch for next N segments
/// - 2.3: Store prefetched segments in cache with appropriate TTL
/// - 2.4: Skip prefetch when buffer is near capacity
pub struct PrefetchManager {
    /// Configuration for prefetching behavior
    config: PrefetchConfig,
    /// Set of segment MSNs that are currently pending prefetch
    pending_prefetch: HashSet<u64>,
    /// Set of segment MSNs that have been completed (downloaded or prefetched)
    completed: HashSet<u64>,
}

impl PrefetchManager {
    /// Create a new PrefetchManager with the given configuration
    pub fn new(config: PrefetchConfig) -> Self {
        Self {
            config,
            pending_prefetch: HashSet::new(),
            completed: HashSet::new(),
        }
    }

    /// Check if prefetching is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get the prefetch count configuration
    pub fn prefetch_count(&self) -> usize {
        self.config.prefetch_count
    }

    /// Determine which segments to prefetch after a successful download.
    ///
    /// This method analyzes the current state and returns a list of segment MSNs
    /// that should be prefetched. It respects buffer limits and avoids prefetching
    /// segments that are already pending or completed.
    ///
    /// # Arguments
    /// * `completed_msn` - The media sequence number of the segment that just completed
    /// * `buffer_size` - Current number of segments in the buffer
    /// * `known_segments` - List of known segment MSNs from the playlist
    ///
    /// # Returns
    /// A vector of segment MSNs to prefetch, sorted in ascending order
    ///
    /// # Requirements
    /// - 2.1: Generate targets for next N segments (where N = prefetch_count)
    /// - 2.3: Only return segments not already in buffer or pending
    /// - 2.4: Return empty list if buffer_size >= max_buffer_before_skip
    pub fn get_prefetch_targets(
        &mut self,
        completed_msn: u64,
        buffer_size: usize,
        known_segments: &[u64],
    ) -> Vec<u64> {
        // If prefetching is disabled, return empty
        if !self.config.enabled {
            return Vec::new();
        }

        // Check buffer limit - skip prefetch if buffer is near capacity (Requirement 2.4)
        if buffer_size >= self.config.max_buffer_before_skip {
            debug!(
                buffer_size = buffer_size,
                max_buffer = self.config.max_buffer_before_skip,
                "Skipping prefetch due to buffer pressure"
            );
            return Vec::new();
        }

        let mut targets = Vec::new();

        // Find segments after the completed one that should be prefetched
        for &msn in known_segments {
            // Only consider segments after the completed one
            if msn <= completed_msn {
                continue;
            }

            // Skip if already pending or completed
            if self.pending_prefetch.contains(&msn) || self.completed.contains(&msn) {
                continue;
            }

            targets.push(msn);

            // Stop when we have enough prefetch targets
            if targets.len() >= self.config.prefetch_count {
                break;
            }
        }

        // Sort targets by MSN (should already be sorted if known_segments is sorted)
        targets.sort();

        // Mark these as pending prefetch
        for &msn in &targets {
            self.pending_prefetch.insert(msn);
        }

        if !targets.is_empty() {
            debug!(
                completed_msn = completed_msn,
                targets = ?targets,
                "Generated prefetch targets"
            );
        }

        targets
    }

    /// Mark a segment as completed (downloaded or prefetched).
    ///
    /// This removes the segment from the pending set and adds it to the completed set.
    ///
    /// # Arguments
    /// * `msn` - The media sequence number of the completed segment
    pub fn mark_completed(&mut self, msn: u64) {
        self.pending_prefetch.remove(&msn);
        self.completed.insert(msn);
    }

    /// Check if a segment is pending prefetch
    pub fn is_pending(&self, msn: u64) -> bool {
        self.pending_prefetch.contains(&msn)
    }

    /// Get the number of pending prefetch requests
    pub fn pending_count(&self) -> usize {
        self.pending_prefetch.len()
    }

    /// Clear all tracking state (useful for discontinuity handling)
    pub fn clear(&mut self) {
        self.pending_prefetch.clear();
        self.completed.clear();
    }

    /// Remove old completed entries to prevent unbounded memory growth.
    ///
    /// This should be called periodically to clean up segments that are
    /// no longer relevant (e.g., segments that have been output).
    ///
    /// # Arguments
    /// * `min_msn` - Remove all completed entries with MSN less than this value
    pub fn cleanup_before(&mut self, min_msn: u64) {
        self.completed.retain(|&msn| msn >= min_msn);
        self.pending_prefetch.retain(|&msn| msn >= min_msn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> PrefetchConfig {
        PrefetchConfig {
            enabled: true,
            prefetch_count: 2,
            max_buffer_before_skip: 40,
        }
    }

    // --- Unit Tests ---

    #[test]
    fn test_prefetch_manager_new() {
        let config = default_config();
        let manager = PrefetchManager::new(config);

        assert!(manager.is_enabled());
        assert_eq!(manager.prefetch_count(), 2);
        assert_eq!(manager.pending_count(), 0);
    }

    #[test]
    fn test_prefetch_manager_disabled() {
        let config = PrefetchConfig {
            enabled: false,
            prefetch_count: 2,
            max_buffer_before_skip: 40,
        };
        let mut manager = PrefetchManager::new(config);

        assert!(!manager.is_enabled());

        // Should return empty when disabled
        let targets = manager.get_prefetch_targets(1, 0, &[1, 2, 3, 4, 5]);
        assert!(targets.is_empty());
    }

    #[test]
    fn test_get_prefetch_targets_basic() {
        let config = default_config();
        let mut manager = PrefetchManager::new(config);

        let known_segments = vec![1, 2, 3, 4, 5];
        let targets = manager.get_prefetch_targets(1, 5, &known_segments);

        // Should return next 2 segments after MSN 1
        assert_eq!(targets, vec![2, 3]);
    }

    #[test]
    fn test_get_prefetch_targets_respects_buffer_limit() {
        let config = PrefetchConfig {
            enabled: true,
            prefetch_count: 2,
            max_buffer_before_skip: 10,
        };
        let mut manager = PrefetchManager::new(config);

        let known_segments = vec![1, 2, 3, 4, 5];

        // Buffer at limit - should skip prefetch
        let targets = manager.get_prefetch_targets(1, 10, &known_segments);
        assert!(targets.is_empty());

        // Buffer over limit - should skip prefetch
        let targets = manager.get_prefetch_targets(1, 15, &known_segments);
        assert!(targets.is_empty());
    }

    #[test]
    fn test_get_prefetch_targets_skips_pending() {
        let config = default_config();
        let mut manager = PrefetchManager::new(config);

        let known_segments = vec![1, 2, 3, 4, 5];

        // First call - should get 2, 3
        let targets1 = manager.get_prefetch_targets(1, 5, &known_segments);
        assert_eq!(targets1, vec![2, 3]);

        // Second call with same completed_msn - should get 4, 5 (2, 3 are pending)
        let targets2 = manager.get_prefetch_targets(1, 5, &known_segments);
        assert_eq!(targets2, vec![4, 5]);
    }

    #[test]
    fn test_get_prefetch_targets_skips_completed() {
        let config = default_config();
        let mut manager = PrefetchManager::new(config);

        let known_segments = vec![1, 2, 3, 4, 5];

        // Mark 2 as completed
        manager.mark_completed(2);

        // Should skip 2 and return 3, 4
        let targets = manager.get_prefetch_targets(1, 5, &known_segments);
        assert_eq!(targets, vec![3, 4]);
    }

    #[test]
    fn test_mark_completed() {
        let config = default_config();
        let mut manager = PrefetchManager::new(config);

        let known_segments = vec![1, 2, 3, 4, 5];

        // Get prefetch targets (marks 2, 3 as pending)
        let targets = manager.get_prefetch_targets(1, 5, &known_segments);
        assert_eq!(targets, vec![2, 3]);
        assert!(manager.is_pending(2));
        assert!(manager.is_pending(3));

        // Mark 2 as completed
        manager.mark_completed(2);
        assert!(!manager.is_pending(2));
        assert!(manager.is_pending(3));
    }

    #[test]
    fn test_cleanup_before() {
        let config = default_config();
        let mut manager = PrefetchManager::new(config);

        // Mark some segments as completed
        manager.mark_completed(1);
        manager.mark_completed(2);
        manager.mark_completed(3);
        manager.mark_completed(4);

        // Cleanup segments before MSN 3
        manager.cleanup_before(3);

        // 1 and 2 should be removed, 3 and 4 should remain
        let known_segments = vec![1, 2, 3, 4, 5, 6];
        let targets = manager.get_prefetch_targets(4, 5, &known_segments);

        // Should return 5, 6 (3 and 4 are still marked as completed)
        assert_eq!(targets, vec![5, 6]);
    }

    #[test]
    fn test_clear() {
        let config = default_config();
        let mut manager = PrefetchManager::new(config);

        let known_segments = vec![1, 2, 3, 4, 5];

        // Get prefetch targets and mark some completed
        manager.get_prefetch_targets(1, 5, &known_segments);
        manager.mark_completed(2);

        // Clear all state
        manager.clear();

        assert_eq!(manager.pending_count(), 0);

        // Should be able to get same targets again
        let targets = manager.get_prefetch_targets(1, 5, &known_segments);
        assert_eq!(targets, vec![2, 3]);
    }

    #[test]
    fn test_prefetch_with_gaps_in_known_segments() {
        let config = default_config();
        let mut manager = PrefetchManager::new(config);

        // Known segments with gaps
        let known_segments = vec![1, 3, 5, 7, 9];

        let targets = manager.get_prefetch_targets(1, 5, &known_segments);

        // Should return next 2 known segments after 1
        assert_eq!(targets, vec![3, 5]);
    }

    #[test]
    fn test_prefetch_fewer_than_count_available() {
        let config = PrefetchConfig {
            enabled: true,
            prefetch_count: 5,
            max_buffer_before_skip: 40,
        };
        let mut manager = PrefetchManager::new(config);

        let known_segments = vec![1, 2, 3];

        // Only 2 segments available after MSN 1
        let targets = manager.get_prefetch_targets(1, 5, &known_segments);
        assert_eq!(targets, vec![2, 3]);
    }
}
