//! # Source Management
//!
//! This module provides functionality for managing multiple content sources.
//! It supports different strategies for source selection, source health tracking,
//! and automatic failover.

use rand::Rng;
use std::collections::HashMap;
use std::fmt::Debug;
use std::time::{Duration, Instant};
use tracing::debug;

/// Strategy for selecting among multiple sources
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceSelectionStrategy {
    /// Select sources in order of priority (lower number = higher priority)
    Priority,
    /// Round-robin selection between sources
    RoundRobin,
    /// Select the source with the fastest response time
    FastestResponse,
    /// Select a random source each time
    Random,
}

impl Default for SourceSelectionStrategy {
    fn default() -> Self {
        Self::Priority
    }
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
            .filter(|s| self.health.get(&s.url).map(|h| h.active).unwrap_or(false))
            .count()
    }

    /// Select a source for the next request
    pub fn select_source(&mut self) -> Option<ContentSource> {
        if self.sources.is_empty() {
            return None;
        }

        if self.sources.len() == 1 {
            // If only one source, return it directly
            return Some(self.sources[0].clone());
        }

        // Select a source based on the strategy
        let source = match self.strategy {
            SourceSelectionStrategy::Priority => self.select_by_priority(),
            SourceSelectionStrategy::RoundRobin => self.select_round_robin(),
            SourceSelectionStrategy::FastestResponse => self.select_fastest(),
            SourceSelectionStrategy::Random => self.select_random(),
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
        // Find the first active source by priority
        self.sources
            .iter()
            .filter(|s| self.health.get(&s.url).map(|h| h.active).unwrap_or(false))
            .min_by_key(|s| s.priority)
            .cloned()
    }

    /// Select a source using round-robin strategy
    fn select_round_robin(&mut self) -> Option<ContentSource> {
        if self.sources.is_empty() {
            return None;
        }

        // Find the next active source in round-robin fashion
        let mut checked = 0;
        let mut index = self.current_index;

        // Loop until we find an active source or checked all sources
        while checked < self.sources.len() {
            let source = &self.sources[index];
            index = (index + 1) % self.sources.len();
            checked += 1;

            let is_active = self
                .health
                .get(&source.url)
                .map(|h| h.active)
                .unwrap_or(false);

            if is_active {
                self.current_index = index;
                return Some(source.clone());
            }
        }

        // If all sources are inactive, try using the original next source
        if checked == self.sources.len() && !self.sources.is_empty() {
            let source = self.sources[self.current_index].clone();
            self.current_index = (self.current_index + 1) % self.sources.len();
            return Some(source);
        }

        None
    }

    /// Select the fastest source
    fn select_fastest(&mut self) -> Option<ContentSource> {
        // Sort sources by response time if needed
        self.sort_sources();

        // Return the fastest active source
        self.sources
            .iter()
            .find(|s| self.health.get(&s.url).map(|h| h.active).unwrap_or(false))
            .cloned()
    }

    /// Select a random source
    fn select_random(&self) -> Option<ContentSource> {
        if self.sources.is_empty() {
            return None;
        }

        // Get active sources
        let active_sources: Vec<_> = self
            .sources
            .iter()
            .filter(|s| self.health.get(&s.url).map(|h| h.active).unwrap_or(false))
            .collect();

        if active_sources.is_empty() {
            // If no active sources, choose from all sources
            let index = rand::rng().random_range(0..self.sources.len());
            Some(self.sources[index].clone())
        } else {
            // Choose a random active source
            let index = rand::rng().random_range(0..active_sources.len());
            Some(active_sources[index].clone())
        }
    }

    /// Record the result of a request to a source
    pub fn record_result(&mut self, url: &str, success: bool, response_time: Duration) {
        let health = self.health.entry(url.to_string()).or_default();

        // Update success/failure counts
        if success {
            health.successes += 1;
        } else {
            health.failures += 1;
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

        // Update active status based on health score
        health.active = health.score > 20;

        debug!(
            url = url,
            success = success,
            response_time_ms = time_ms,
            avg_response_time_ms = health.avg_response_time,
            health_score = health.score,
            active = health.active,
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

        // Final score is weighted average: 70% success rate, 30% time score
        health.score = ((success_rate as u32 * 70 + time_score as u32 * 30) / 100) as u8;
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
