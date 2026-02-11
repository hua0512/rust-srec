//! # Source Management
//!
//! This module provides functionality for managing multiple content sources.
//! It supports different strategies for source selection, source health tracking,
//! and automatic failover.

use crate::DownloadError;
use rand::Rng;
use std::collections::HashMap;
use std::fmt::Debug;
use std::time::{Duration, Instant};
use tracing::{debug, trace};

/// Strategy for selecting among multiple sources
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SourceSelectionStrategy {
    /// Select sources in order of priority (lower number = higher priority)
    #[default]
    Priority,
    /// Round-robin selection between sources
    RoundRobin,
    /// Select the source with the fastest response time
    FastestResponse,
    /// Select a random source each time
    Random,
}

/// Source health status tracking
#[derive(Debug, Clone)]
struct SourceHealth {
    /// Number of successful requests
    successes: u32,
    /// Number of failed requests
    failures: u32,
    /// Average response time in milliseconds
    avg_response_time: u64,
    /// When the source was last used
    last_used: Option<Instant>,
    /// Current health score (0-100)
    score: u8,
    /// Whether the source is currently considered active
    active: bool,
    /// Number of consecutive failures
    consecutive_failures: u32,
    /// When the source was temporarily disabled (for circuit breaker)
    disabled_until: Option<Instant>,
}

impl Default for SourceHealth {
    fn default() -> Self {
        Self {
            successes: 0,
            failures: 0,
            avg_response_time: 0,
            last_used: None,
            score: 100, // Start with full health
            active: true,
            consecutive_failures: 0,
            disabled_until: None,
        }
    }
}

/// Represents a content source (URL) with priority
#[derive(Debug, Clone)]
pub struct ContentSource {
    /// The URL of the content source
    pub url: String,
    /// Priority of the source (lower number = higher priority)
    pub priority: u8,
    /// Human-readable label for this source
    pub label: Option<String>,
    /// Optional geographic location information
    pub location: Option<String>,
}

impl ContentSource {
    /// Create a new content source with the given URL and priority
    pub fn new(url: impl Into<String>, priority: u8) -> Self {
        Self {
            url: url.into(),
            priority,
            label: None,
            location: None,
        }
    }

    /// Set a label for this source
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set a location for this source
    pub fn with_location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }
}

/// Manager for handling multiple content sources
#[derive(Debug)]
pub struct SourceManager {
    /// Available content sources
    sources: Vec<ContentSource>,
    /// Health tracking for each source
    health: HashMap<String, SourceHealth>,
    /// Selection strategy
    strategy: SourceSelectionStrategy,
    /// Index for round-robin strategy
    current_index: usize,
    /// History of last selected sources (to avoid consecutive failures)
    recent_selections: Vec<String>,
}

impl Default for SourceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SourceManager {
    /// Create a new source manager with default strategy (Priority)
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
            health: HashMap::new(),
            strategy: SourceSelectionStrategy::default(),
            current_index: 0,
            recent_selections: Vec::with_capacity(3),
        }
    }

    /// Create a new source manager with the specified strategy
    pub fn with_strategy(strategy: SourceSelectionStrategy) -> Self {
        Self {
            sources: Vec::new(),
            health: HashMap::new(),
            strategy,
            current_index: 0,
            recent_selections: Vec::with_capacity(3),
        }
    }

    /// Add a content source
    pub fn add_source(&mut self, source: ContentSource) {
        // Initialize health tracking for this source
        self.health.entry(source.url.clone()).or_default();

        // Add the source
        self.sources.push(source);

        // Sort sources by priority
        self.sort_sources();
    }

    /// Add a URL as a content source with the given priority
    pub fn add_url(&mut self, url: impl Into<String>, priority: u8) {
        let url_str = url.into();
        self.add_source(ContentSource::new(url_str, priority));
    }

    /// Check if a source is available (active and not in circuit breaker state)
    fn is_source_available(&self, url: &str) -> bool {
        match self.health.get(url) {
            Some(health) => {
                // Check if source is active
                if !health.active {
                    return false;
                }

                // Check circuit breaker state
                if let Some(disabled_until) = health.disabled_until
                    && Instant::now() < disabled_until
                {
                    return false; // Still in circuit breaker timeout
                }

                true
            }
            None => true, // New source, assume available
        }
    }

    /// Sort sources according to the current strategy
    fn sort_sources(&mut self) {
        match self.strategy {
            SourceSelectionStrategy::Priority => {
                // Sort by priority (lower number = higher priority)
                self.sources.sort_by_key(|s| s.priority);
            }
            SourceSelectionStrategy::FastestResponse => {
                // Sort by average response time (lower = faster = better)
                self.sources.sort_by_key(|s| {
                    self.health
                        .get(&s.url)
                        .map(|h| h.avg_response_time)
                        .unwrap_or(u64::MAX)
                });
            }
            // For RoundRobin and Random, no sorting needed
            _ => {}
        }
    }

    /// Check if there are any sources configured
    pub fn has_sources(&self) -> bool {
        !self.sources.is_empty()
    }

    /// Get the number of configured sources
    pub fn count(&self) -> usize {
        self.sources.len()
    }

    /// Get the number of healthy sources
    pub fn healthy_count(&self) -> usize {
        self.sources
            .iter()
            .filter(|s| self.is_source_available(&s.url))
            .count()
    }

    /// Select a source for the next request
    pub fn select_source(&mut self) -> Option<ContentSource> {
        if self.sources.is_empty() {
            return None;
        }

        let source = if self.sources.len() == 1 {
            let source = &self.sources[0];
            self.is_source_available(&source.url)
                .then(|| source.clone())
        } else {
            // Select a source based on the strategy
            match self.strategy {
                SourceSelectionStrategy::Priority => self.select_by_priority(),
                SourceSelectionStrategy::RoundRobin => self.select_round_robin(),
                SourceSelectionStrategy::FastestResponse => self.select_fastest(),
                SourceSelectionStrategy::Random => self.select_random(),
            }
        };

        // Update the recent selections list
        if let Some(ref src) = source {
            // If we've reached capacity, remove oldest
            if self.recent_selections.len() >= 3 {
                self.recent_selections.remove(0);
            }
            self.recent_selections.push(src.url.clone());

            // Mark the source as used
            if let Some(health) = self.health.get_mut(&src.url) {
                health.last_used = Some(Instant::now());
            }
        }

        source
    }

    /// Select a source using the priority strategy
    fn select_by_priority(&self) -> Option<ContentSource> {
        // Sources are kept sorted by priority.
        self.sources
            .iter()
            .find(|s| self.is_source_available(&s.url))
            .cloned()
    }

    /// Select a source using round-robin strategy
    fn select_round_robin(&mut self) -> Option<ContentSource> {
        if self.sources.is_empty() {
            return None;
        }

        // Find the next available source in round-robin fashion
        let mut checked = 0;
        let mut index = self.current_index;

        // Loop until we find an available source or checked all sources
        while checked < self.sources.len() {
            let source = &self.sources[index];
            index = (index + 1) % self.sources.len();
            checked += 1;

            if self.is_source_available(&source.url) {
                self.current_index = index;
                return Some(source.clone());
            }
        }

        None
    }

    /// Select the fastest source
    fn select_fastest(&mut self) -> Option<ContentSource> {
        // Sort sources by response time if needed
        self.sort_sources();

        // Return the fastest available source
        self.sources
            .iter()
            .find(|s| self.is_source_available(&s.url))
            .cloned()
    }

    /// Select a random source
    fn select_random(&self) -> Option<ContentSource> {
        // Reservoir sample a uniformly random available source without allocating.
        let mut rng = rand::rng();
        let mut chosen: Option<&ContentSource> = None;

        for (index, source) in self
            .sources
            .iter()
            .filter(|s| self.is_source_available(&s.url))
            .enumerate()
        {
            let seen = index + 1;
            if rng.random_range(0..seen) == 0 {
                chosen = Some(source);
            }
        }

        chosen.cloned()
    }

    /// Record a successful request to a source
    pub fn record_success(&mut self, url: &str, response_time: Duration) {
        self.record_result(url, true, response_time);
    }

    /// Check if an error indicates a non-recoverable condition for a source
    fn is_non_recoverable_error(error: &DownloadError) -> bool {
        match error {
            DownloadError::Network { source } => {
                source.status().is_some_and(|s| s.is_client_error())
            }
            _ => error.is_non_recoverable_source_error(),
        }
    }

    /// Record a failed request to a source and update health
    pub fn record_failure(&mut self, url: &str, error: &DownloadError, response_time: Duration) {
        // Deactivate source permanently for non-recoverable errors
        if Self::is_non_recoverable_error(error) {
            self.set_source_active(url, false);
            return;
        }

        self.record_result(url, false, response_time);
    }

    /// Record the result of a request to a source
    fn record_result(&mut self, url: &str, success: bool, response_time: Duration) {
        let health = self.health.entry(url.to_string()).or_default();

        // Update success/failure counts
        if success {
            health.successes += 1;
            health.consecutive_failures = 0; // Reset consecutive failures on success
            health.disabled_until = None; // Clear any circuit breaker state
        } else {
            health.failures += 1;
            health.consecutive_failures += 1;
        }

        // Update response time with weighted average
        let time_ms = response_time.as_millis() as u64;
        if health.avg_response_time == 0 {
            health.avg_response_time = time_ms;
        } else {
            // 70% old value, 30% new value for smoothing
            health.avg_response_time = (health.avg_response_time * 7 + time_ms * 3) / 10;
        }

        // Calculate health score
        Self::calculate_health_score(health);

        // Circuit breaker logic: disable source temporarily after repeated failures
        if !success && health.consecutive_failures >= 3 {
            const MAX_BACKOFF_SECS: u64 = 300; // 5 minutes

            // Safe: consecutive_failures >= 3, so subtraction cannot underflow.
            // Cap the exponent to avoid overflow and excessively long backoffs.
            let exp = (health.consecutive_failures - 3).min(31);
            let backoff_secs = (1_u64.checked_shl(exp).unwrap_or(u64::MAX)).min(MAX_BACKOFF_SECS);
            let backoff_duration = Duration::from_secs(backoff_secs);
            health.disabled_until = Some(Instant::now() + backoff_duration);

            debug!(
                url = url,
                consecutive_failures = health.consecutive_failures,
                backoff_seconds = backoff_duration.as_secs(),
                "Source temporarily disabled due to consecutive failures"
            );
        }

        // Update active status based on health score (but not too restrictive for fast failures)
        health.active = if health.successes == 0 && health.failures > 0 {
            // If we have only failures, deactivate permanently after many attempts
            health.failures < 10
        } else {
            // Normal health score calculation
            health.score > 20
        };

        trace!(
            url = url,
            success = success,
            response_time_ms = time_ms,
            avg_response_time_ms = health.avg_response_time,
            health_score = health.score,
            active = health.active,
            consecutive_failures = health.consecutive_failures,
            "Source health updated"
        );

        // If the strategy depends on health metrics, re-sort the sources
        if self.strategy == SourceSelectionStrategy::FastestResponse {
            self.sort_sources();
        }
    }

    // /// Update the health score for a source
    // fn update_health_score(&mut self, url: &str) {
    //     let health = match self.health.get_mut(url) {
    //         Some(h) => h,
    //         None => return,
    //     };

    //     Self::calculate_health_score(health);
    // }

    /// Calculate and update health score for a health record
    fn calculate_health_score(health: &mut SourceHealth) {
        let total = health.successes + health.failures;
        if total == 0 {
            health.score = 100;
            return;
        }

        // Calculate success rate (0-100)
        let success_rate = (health.successes as f32 * 100.0 / total as f32) as u8;

        // If success rate is 0%, the health score should be very low regardless of response time
        if success_rate == 0 {
            // Penalize consecutive failures even more
            let consecutive_penalty = (health.consecutive_failures * 5).min(255) as u8;
            health.score = (10_u8).saturating_sub(consecutive_penalty);
            return;
        }

        // Response time score (faster = better)
        // 0ms - 100ms: 100-80
        // 100ms - 500ms: 80-60
        // 500ms - 1s: 60-40
        // >1s: <40
        let time_score = if health.avg_response_time < 100 {
            80 + (20 * (100 - health.avg_response_time) / 100) as u8
        } else if health.avg_response_time < 500 {
            60 + (20 * (500 - health.avg_response_time) / 400) as u8
        } else if health.avg_response_time < 1000 {
            40 + (20 * (1000 - health.avg_response_time) / 500) as u8
        } else {
            (40 * 1000 / health.avg_response_time.max(1)) as u8
        };

        // For successful sources, final score is weighted average: 80% success rate, 20% time score
        // This gives more weight to reliability than speed
        health.score = ((success_rate as u32 * 80 + time_score as u32 * 20) / 100) as u8;
    }

    /// Manually set the active status of a source
    pub fn set_source_active(&mut self, url: &str, active: bool) {
        if let Some(health) = self.health.get_mut(url) {
            health.active = active;
            debug!(
                url = url,
                active = active,
                "Source active status updated manually"
            );
        }
    }

    /// Clear all source health data
    pub fn reset_health(&mut self) {
        self.health.clear();
        for source in &self.sources {
            self.health
                .insert(source.url.clone(), SourceHealth::default());
        }
    }

    /// Get the current health information for a source
    pub fn get_source_health(&self, url: &str) -> Option<(u8, u64, bool)> {
        // Returns (health score, avg response time, active status)
        self.health
            .get(url)
            .map(|h| (h.score, h.avg_response_time, h.active))
    }

    /// Get a list of all sources with their health information
    pub fn get_all_sources_health(&self) -> Vec<(String, u8, u64, bool)> {
        self.sources
            .iter()
            .filter_map(|s| {
                self.health
                    .get(&s.url)
                    .map(|h| (s.url.clone(), h.score, h.avg_response_time, h.active))
            })
            .collect()
    }

    /// Change the source selection strategy
    pub fn set_strategy(&mut self, strategy: SourceSelectionStrategy) {
        self.strategy = strategy;
        self.sort_sources();
    }

    /// Get the current source selection strategy
    pub fn get_strategy(&self) -> &SourceSelectionStrategy {
        &self.strategy
    }
}
