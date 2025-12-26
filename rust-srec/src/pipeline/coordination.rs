//! Session coordination for collecting DAG outputs and triggering session-complete pipelines.

use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;
use tracing::{debug, info, warn};

/// Source type for segment outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceType {
    Video,
    Danmu,
}

/// Output from a segment with ordering information.
#[derive(Debug, Clone)]
pub struct SegmentOutput {
    /// Segment index for ordering (important for concat)
    pub segment_index: u32,
    /// Path to the output file
    pub path: PathBuf,
}

/// Collected outputs for a session.
#[derive(Debug)]
pub struct SessionOutputs {
    pub session_id: String,
    pub streamer_id: String,
    pub danmu_enabled: bool,
    pub created_at: Instant,

    /// Video segment outputs (from per-segment DAGs)
    pub video_outputs: Vec<SegmentOutput>,
    /// Danmu segment outputs (from per-segment DAGs)
    pub danmu_outputs: Vec<SegmentOutput>,

    /// Stream completion flags
    pub video_complete: bool,
    pub danmu_complete: bool,

    /// Pending DAG counters
    pub pending_video_dags: AtomicU32,
    pub pending_danmu_dags: AtomicU32,
}

impl SessionOutputs {
    pub fn new(session_id: String, streamer_id: String, danmu_enabled: bool) -> Self {
        Self {
            session_id,
            streamer_id,
            danmu_enabled,
            created_at: Instant::now(),
            video_outputs: Vec::new(),
            danmu_outputs: Vec::new(),
            video_complete: false,
            danmu_complete: false,
            pending_video_dags: AtomicU32::new(0),
            pending_danmu_dags: AtomicU32::new(0),
        }
    }

    /// Check if session is ready for session-complete pipeline.
    pub fn is_ready(&self) -> bool {
        let video_ready =
            self.video_complete && self.pending_video_dags.load(Ordering::SeqCst) == 0;
        let danmu_ready = !self.danmu_enabled
            || (self.danmu_complete && self.pending_danmu_dags.load(Ordering::SeqCst) == 0);

        video_ready && danmu_ready
    }

    /// Get all outputs sorted by segment index.
    pub fn get_sorted_video_outputs(&self) -> Vec<PathBuf> {
        let mut outputs = self.video_outputs.to_vec();
        outputs.sort_by_key(|o| o.segment_index);
        outputs.into_iter().map(|o| o.path).collect()
    }

    /// Get all danmu outputs sorted by segment index.
    pub fn get_sorted_danmu_outputs(&self) -> Vec<PathBuf> {
        let mut outputs = self.danmu_outputs.to_vec();
        outputs.sort_by_key(|o| o.segment_index);
        outputs.into_iter().map(|o| o.path).collect()
    }
}

/// Coordinator for session-complete pipeline triggering.
///
/// Tracks outputs from per-segment DAGs and triggers session-complete
/// pipeline when all conditions are met:
/// - DownloadCompleted received
/// - CollectionStopped received (if danmu enabled)
/// - All pending per-segment DAGs completed
pub struct SessionCompleteCoordinator {
    sessions: DashMap<String, SessionOutputs>,
}

impl SessionCompleteCoordinator {
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
        }
    }

    /// Initialize session tracking.
    pub fn init_session(&self, session_id: &str, streamer_id: &str, danmu_enabled: bool) {
        if !self.sessions.contains_key(session_id) {
            debug!(
                session_id = %session_id,
                streamer_id = %streamer_id,
                danmu_enabled = %danmu_enabled,
                "Initializing session tracking"
            );
            self.sessions.insert(
                session_id.to_string(),
                SessionOutputs::new(
                    session_id.to_string(),
                    streamer_id.to_string(),
                    danmu_enabled,
                ),
            );
        }
    }

    /// Called when a per-segment DAG is about to start.
    pub fn on_dag_started(&self, session_id: &str, source: SourceType) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            match source {
                SourceType::Video => {
                    session.pending_video_dags.fetch_add(1, Ordering::SeqCst);
                }
                SourceType::Danmu => {
                    session.pending_danmu_dags.fetch_add(1, Ordering::SeqCst);
                }
            }
            debug!(
                session_id = %session_id,
                source = ?source,
                "DAG started, incremented pending count"
            );
        }
    }

    /// Called when a per-segment DAG completes with outputs.
    pub fn on_dag_complete(
        &self,
        session_id: &str,
        segment_index: u32,
        outputs: Vec<PathBuf>,
        source: SourceType,
    ) {
        if let Some(mut session) = self.sessions.get_mut(session_id) {
            // Decrement pending counter
            match source {
                SourceType::Video => {
                    session.pending_video_dags.fetch_sub(1, Ordering::SeqCst);
                }
                SourceType::Danmu => {
                    session.pending_danmu_dags.fetch_sub(1, Ordering::SeqCst);
                }
            }

            // Collect outputs (skip if empty - e.g., delete/upload-move DAGs)
            if outputs.is_empty() {
                debug!(
                    session_id = %session_id,
                    segment_index = %segment_index,
                    source = ?source,
                    "DAG completed with no outputs, not collecting"
                );
            } else {
                for path in outputs {
                    let output = SegmentOutput {
                        segment_index,
                        path,
                    };
                    match source {
                        SourceType::Video => session.video_outputs.push(output),
                        SourceType::Danmu => session.danmu_outputs.push(output),
                    }
                }
                debug!(
                    session_id = %session_id,
                    segment_index = %segment_index,
                    source = ?source,
                    "DAG completed, collected outputs"
                );
            }
        }
    }

    /// Called when a per-segment DAG fails.
    pub fn on_dag_failed(&self, session_id: &str, source: SourceType) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            match source {
                SourceType::Video => {
                    session.pending_video_dags.fetch_sub(1, Ordering::SeqCst);
                }
                SourceType::Danmu => {
                    session.pending_danmu_dags.fetch_sub(1, Ordering::SeqCst);
                }
            }
            warn!(
                session_id = %session_id,
                source = ?source,
                "DAG failed, continuing with partial outputs"
            );
        }
    }

    /// Called when no per-segment DAG is configured - collect raw segment.
    pub fn on_raw_segment(
        &self,
        session_id: &str,
        segment_index: u32,
        path: PathBuf,
        source: SourceType,
    ) {
        if let Some(mut session) = self.sessions.get_mut(session_id) {
            let output = SegmentOutput {
                segment_index,
                path,
            };
            match source {
                SourceType::Video => session.video_outputs.push(output),
                SourceType::Danmu => session.danmu_outputs.push(output),
            }
            debug!(
                session_id = %session_id,
                segment_index = %segment_index,
                source = ?source,
                "Raw segment collected (no DAG configured)"
            );
        }
    }

    /// Called when video stream completes (DownloadCompleted).
    pub fn on_video_complete(&self, session_id: &str) {
        if let Some(mut session) = self.sessions.get_mut(session_id) {
            session.video_complete = true;
            info!(
                session_id = %session_id,
                "Video stream completed"
            );
        }
    }

    /// Called when danmu collection stops (CollectionStopped).
    pub fn on_danmu_complete(&self, session_id: &str) {
        if let Some(mut session) = self.sessions.get_mut(session_id) {
            session.danmu_complete = true;
            info!(
                session_id = %session_id,
                "Danmu collection stopped"
            );
        }
    }

    /// Try to trigger session-complete pipeline.
    /// Returns outputs if ready, None otherwise.
    pub fn try_trigger(&self, session_id: &str) -> Option<SessionOutputs> {
        // Check if ready without holding the lock
        if let Some(session) = self.sessions.get(session_id) {
            if !session.is_ready() {
                return None;
            }
        } else {
            return None;
        }

        // Remove and return the session
        if let Some((_, outputs)) = self.sessions.remove(session_id) {
            if outputs.video_outputs.is_empty() {
                warn!(
                    session_id = %session_id,
                    "Session completed with no video outputs, skipping pipeline"
                );
                return None;
            }
            info!(
                session_id = %session_id,
                video_outputs = %outputs.video_outputs.len(),
                danmu_outputs = %outputs.danmu_outputs.len(),
                "Session ready for session-complete pipeline"
            );
            Some(outputs)
        } else {
            None
        }
    }

    /// Cleanup stale sessions (TTL-based).
    pub fn cleanup_stale(&self, max_age_secs: u64) {
        let max_age = std::time::Duration::from_secs(max_age_secs);
        let mut removed = 0;

        self.sessions.retain(|session_id, session| {
            if session.created_at.elapsed() > max_age {
                warn!(
                    session_id = %session_id,
                    age_secs = %session.created_at.elapsed().as_secs(),
                    "Removing stale session"
                );
                removed += 1;
                false
            } else {
                true
            }
        });

        if removed > 0 {
            info!(removed = %removed, "Cleaned up stale sessions");
        }
    }

    /// Get number of active sessions (for metrics).
    pub fn active_session_count(&self) -> usize {
        self.sessions.len()
    }
}

impl Default for SessionCompleteCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_init_and_complete() {
        let coord = SessionCompleteCoordinator::new();

        coord.init_session("session1", "streamer1", false);

        // Not ready - video not complete
        assert!(coord.try_trigger("session1").is_none());

        // Add a raw segment
        coord.on_raw_segment(
            "session1",
            0,
            PathBuf::from("/test/seg0.mp4"),
            SourceType::Video,
        );

        // Still not ready
        assert!(coord.try_trigger("session1").is_none());

        // Mark video complete
        coord.on_video_complete("session1");

        // Now should be ready (danmu disabled)
        let outputs = coord.try_trigger("session1").expect("Should be ready");
        assert_eq!(outputs.video_outputs.len(), 1);
    }

    #[test]
    fn test_dag_tracking() {
        let coord = SessionCompleteCoordinator::new();

        coord.init_session("session1", "streamer1", false);

        // Start a DAG
        coord.on_dag_started("session1", SourceType::Video);

        // Mark stream complete
        coord.on_video_complete("session1");

        // Not ready - DAG still pending
        assert!(coord.try_trigger("session1").is_none());

        // Complete the DAG
        coord.on_dag_complete(
            "session1",
            0,
            vec![PathBuf::from("/out.mp4")],
            SourceType::Video,
        );

        // Now ready
        let outputs = coord.try_trigger("session1").expect("Should be ready");
        assert_eq!(outputs.video_outputs.len(), 1);
    }

    #[test]
    fn test_danmu_enabled() {
        let coord = SessionCompleteCoordinator::new();

        coord.init_session("session1", "streamer1", true); // danmu enabled

        coord.on_raw_segment("session1", 0, PathBuf::from("/seg.mp4"), SourceType::Video);
        coord.on_video_complete("session1");

        // Not ready - waiting for danmu
        assert!(coord.try_trigger("session1").is_none());

        coord.on_danmu_complete("session1");

        // Now ready
        assert!(coord.try_trigger("session1").is_some());
    }

    #[test]
    fn test_output_ordering() {
        let coord = SessionCompleteCoordinator::new();

        coord.init_session("session1", "streamer1", false);

        // Add segments out of order
        coord.on_raw_segment("session1", 2, PathBuf::from("/seg2.mp4"), SourceType::Video);
        coord.on_raw_segment("session1", 0, PathBuf::from("/seg0.mp4"), SourceType::Video);
        coord.on_raw_segment("session1", 1, PathBuf::from("/seg1.mp4"), SourceType::Video);

        coord.on_video_complete("session1");

        let outputs = coord.try_trigger("session1").expect("Should be ready");
        let sorted = outputs.get_sorted_video_outputs();

        assert_eq!(sorted[0], PathBuf::from("/seg0.mp4"));
        assert_eq!(sorted[1], PathBuf::from("/seg1.mp4"));
        assert_eq!(sorted[2], PathBuf::from("/seg2.mp4"));
    }

    #[test]
    fn test_empty_dag_outputs() {
        let coord = SessionCompleteCoordinator::new();

        coord.init_session("session1", "streamer1", false);

        coord.on_dag_started("session1", SourceType::Video);
        coord.on_dag_complete("session1", 0, vec![], SourceType::Video); // Empty outputs (delete DAG)

        coord.on_video_complete("session1");

        // Ready but no outputs -> None (skipped)
        assert!(coord.try_trigger("session1").is_none());
    }
}
