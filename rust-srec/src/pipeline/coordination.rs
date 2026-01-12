//! Session coordination for collecting DAG outputs and triggering session-complete pipelines.

use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::{debug, info, warn};

/// Source type for segment outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceType {
    Video,
    Danmu,
}

fn dedup_paths_preserve_order(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    use std::collections::HashSet;

    fn key(p: &Path) -> String {
        let s = p.to_string_lossy();
        if cfg!(windows) {
            s.to_lowercase()
        } else {
            s.to_string()
        }
    }

    let mut seen = HashSet::<String>::new();
    let mut out = Vec::new();
    for p in paths {
        if seen.insert(key(&p)) {
            out.push(p);
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SegmentKey {
    session_id: String,
    segment_index: u32,
}

impl SegmentKey {
    fn new(session_id: &str, segment_index: u32) -> Self {
        Self {
            session_id: session_id.to_string(),
            segment_index,
        }
    }
}

/// Output from a segment with ordering information.
#[derive(Debug, Clone)]
pub struct SegmentOutput {
    /// Segment index for ordering (important for concat)
    pub segment_index: u32,
    /// Path to the output file
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct PairedSegmentOutputs {
    pub session_id: String,
    pub streamer_id: String,
    pub segment_index: u32,
    pub video_outputs: Vec<PathBuf>,
    pub danmu_outputs: Vec<PathBuf>,
}

#[derive(Debug)]
struct PairedSegmentState {
    session_id: String,
    streamer_id: String,
    segment_index: u32,
    last_activity: Instant,
    video_outputs: Option<Vec<PathBuf>>,
    danmu_outputs: Option<Vec<PathBuf>>,
}

impl PairedSegmentState {
    fn new(session_id: String, streamer_id: String, segment_index: u32) -> Self {
        let now = Instant::now();
        Self {
            session_id,
            streamer_id,
            segment_index,
            last_activity: now,
            video_outputs: None,
            danmu_outputs: None,
        }
    }

    fn is_ready(&self) -> bool {
        self.video_outputs.is_some() && self.danmu_outputs.is_some()
    }
}

/// Coordinator for paired per-segment pipelines.
///
/// Tracks outputs for a `(session_id, segment_index)` from both `Video` and `Danmu` sources.
/// Once both are available, returns them and removes the entry to avoid duplicate triggering.
pub struct PairedSegmentCoordinator {
    segments: DashMap<SegmentKey, PairedSegmentState>,
}

impl PairedSegmentCoordinator {
    pub fn new() -> Self {
        Self {
            segments: DashMap::new(),
        }
    }

    pub fn on_video_ready(
        &self,
        session_id: &str,
        streamer_id: &str,
        segment_index: u32,
        outputs: Vec<PathBuf>,
    ) -> Option<PairedSegmentOutputs> {
        self.on_ready(
            session_id,
            streamer_id,
            segment_index,
            SourceType::Video,
            outputs,
        )
    }

    pub fn on_danmu_ready(
        &self,
        session_id: &str,
        streamer_id: &str,
        segment_index: u32,
        outputs: Vec<PathBuf>,
    ) -> Option<PairedSegmentOutputs> {
        self.on_ready(
            session_id,
            streamer_id,
            segment_index,
            SourceType::Danmu,
            outputs,
        )
    }

    fn on_ready(
        &self,
        session_id: &str,
        streamer_id: &str,
        segment_index: u32,
        source: SourceType,
        outputs: Vec<PathBuf>,
    ) -> Option<PairedSegmentOutputs> {
        let outputs = dedup_paths_preserve_order(outputs);
        if outputs.is_empty() {
            debug!(
                session_id = %session_id,
                segment_index = %segment_index,
                source = ?source,
                "Ignoring paired-segment source with empty outputs"
            );
            return None;
        }

        let key = SegmentKey::new(session_id, segment_index);
        let now = Instant::now();
        {
            let mut entry = self.segments.entry(key.clone()).or_insert_with(|| {
                PairedSegmentState::new(
                    session_id.to_string(),
                    streamer_id.to_string(),
                    segment_index,
                )
            });

            if entry.streamer_id != streamer_id {
                warn!(
                    session_id = %session_id,
                    segment_index = %segment_index,
                    existing_streamer_id = %entry.streamer_id,
                    new_streamer_id = %streamer_id,
                    "Paired segment streamer_id mismatch (keeping existing)"
                );
            }

            entry.last_activity = now;

            match source {
                SourceType::Video => entry.video_outputs = Some(outputs),
                SourceType::Danmu => entry.danmu_outputs = Some(outputs),
            }
        }

        let ready = self
            .segments
            .get(&key)
            .map(|v| v.is_ready())
            .unwrap_or(false);

        if !ready {
            return None;
        }

        let (_, state) = self.segments.remove(&key)?;

        Some(PairedSegmentOutputs {
            session_id: state.session_id,
            streamer_id: state.streamer_id,
            segment_index: state.segment_index,
            video_outputs: state.video_outputs.unwrap_or_default(),
            danmu_outputs: state.danmu_outputs.unwrap_or_default(),
        })
    }

    pub fn cleanup_stale(&self, max_age_secs: u64) {
        let max_age = std::time::Duration::from_secs(max_age_secs);
        let mut removed = 0;

        self.segments.retain(|key, entry| {
            if entry.last_activity.elapsed() > max_age {
                warn!(
                    session_id = %key.session_id,
                    segment_index = %key.segment_index,
                    age_secs = %entry.last_activity.elapsed().as_secs(),
                    "Removing stale paired-segment entry"
                );
                removed += 1;
                false
            } else {
                true
            }
        });

        if removed > 0 {
            info!(removed = %removed, "Cleaned up stale paired-segment entries");
        }
    }

    #[cfg(test)]
    pub fn active_pair_count(&self) -> usize {
        self.segments.len()
    }
}

impl Default for PairedSegmentCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

/// Collected outputs for a session.
#[derive(Debug)]
pub struct SessionOutputs {
    pub session_id: String,
    pub streamer_id: String,
    /// Whether danmu was configured/expected for this session.
    ///
    /// Note: we still gate on actual danmu activity via `danmu_observed` to avoid
    /// permanently blocking sessions where danmu never started.
    pub danmu_expected: bool,
    /// Whether we observed any danmu activity for this session (start/segment/DAG/stop).
    pub danmu_observed: bool,
    pub created_at: Instant,
    pub last_activity: Instant,

    /// Video segment outputs (from per-segment DAGs)
    pub video_outputs: Vec<SegmentOutput>,
    /// Danmu segment outputs (from per-segment DAGs)
    pub danmu_outputs: Vec<SegmentOutput>,

    /// Stream completion flags
    pub video_complete: bool,
    pub danmu_complete: bool,

    /// Pending DAG counters
    pub pending_video_dags: u32,
    pub pending_danmu_dags: u32,
    /// Pending paired-segment DAGs (fan-in after both video+danmu are ready).
    pub pending_paired_dags: u32,
}

impl SessionOutputs {
    pub fn new(session_id: String, streamer_id: String, danmu_enabled: bool) -> Self {
        let now = Instant::now();
        Self {
            session_id,
            streamer_id,
            danmu_expected: danmu_enabled,
            danmu_observed: false,
            created_at: now,
            last_activity: now,
            video_outputs: Vec::new(),
            danmu_outputs: Vec::new(),
            video_complete: false,
            danmu_complete: false,
            pending_video_dags: 0,
            pending_danmu_dags: 0,
            pending_paired_dags: 0,
        }
    }

    /// Check if session is ready for session-complete pipeline.
    pub fn is_ready(&self) -> bool {
        let video_ready = self.video_complete && self.pending_video_dags == 0;
        let danmu_required = self.danmu_expected && self.danmu_observed;
        let danmu_ready = !danmu_required || (self.danmu_complete && self.pending_danmu_dags == 0);
        let paired_ready = self.pending_paired_dags == 0;

        video_ready && danmu_ready && paired_ready
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
        match self.sessions.entry(session_id.to_string()) {
            Entry::Occupied(mut entry) => {
                let session = entry.get_mut();
                session.last_activity = Instant::now();
                if session.streamer_id != streamer_id {
                    warn!(
                        session_id = %session_id,
                        existing_streamer_id = %session.streamer_id,
                        new_streamer_id = %streamer_id,
                        "Session tracking streamer_id mismatch (keeping existing)"
                    );
                }
                // If any source indicates danmu should be tracked, keep it enabled.
                if danmu_enabled && !session.danmu_expected {
                    debug!(
                        session_id = %session_id,
                        "Enabling danmu tracking for existing session"
                    );
                    session.danmu_expected = true;
                }
            }
            Entry::Vacant(entry) => {
                debug!(
                    session_id = %session_id,
                    streamer_id = %streamer_id,
                    danmu_enabled = %danmu_enabled,
                    "Initializing session tracking"
                );
                entry.insert(SessionOutputs::new(
                    session_id.to_string(),
                    streamer_id.to_string(),
                    danmu_enabled,
                ));
            }
        }
    }

    /// Mark danmu as started/observed for this session.
    pub fn on_danmu_started(&self, session_id: &str) {
        if let Some(mut session) = self.sessions.get_mut(session_id) {
            session.last_activity = Instant::now();
            session.danmu_observed = true;
            debug!(session_id = %session_id, "Danmu collection started (observed)");
        } else {
            warn!(
                session_id = %session_id,
                "Danmu collection started for unknown session (init_session may be missing)"
            );
        }
    }

    /// Called when a paired-segment DAG (video+danmu fan-in pipeline) is about to start.
    pub fn on_paired_dag_started(&self, session_id: &str) {
        if let Some(mut session) = self.sessions.get_mut(session_id) {
            session.last_activity = Instant::now();
            session.pending_paired_dags = session.pending_paired_dags.saturating_add(1);
            debug!(
                session_id = %session_id,
                pending_paired_dags = %session.pending_paired_dags,
                "Paired-segment DAG started"
            );
        } else {
            warn!(
                session_id = %session_id,
                "Paired-segment DAG started for unknown session (init_session may be missing)"
            );
        }
    }

    /// Called when a paired-segment DAG completes (success).
    pub fn on_paired_dag_complete(&self, session_id: &str) {
        if let Some(mut session) = self.sessions.get_mut(session_id) {
            session.last_activity = Instant::now();
            if session.pending_paired_dags == 0 {
                warn!(
                    session_id = %session_id,
                    "Paired-segment DAG completed but pending counter is already 0"
                );
            } else {
                session.pending_paired_dags -= 1;
            }
            debug!(
                session_id = %session_id,
                pending_paired_dags = %session.pending_paired_dags,
                "Paired-segment DAG completed"
            );
        } else {
            warn!(
                session_id = %session_id,
                "Paired-segment DAG completed for unknown session (init_session may be missing)"
            );
        }
    }

    /// Called when a paired-segment DAG fails (fail-fast or processor error).
    pub fn on_paired_dag_failed(&self, session_id: &str) {
        if let Some(mut session) = self.sessions.get_mut(session_id) {
            session.last_activity = Instant::now();
            if session.pending_paired_dags == 0 {
                warn!(
                    session_id = %session_id,
                    "Paired-segment DAG failed but pending counter is already 0"
                );
            } else {
                session.pending_paired_dags -= 1;
            }
            warn!(
                session_id = %session_id,
                pending_paired_dags = %session.pending_paired_dags,
                "Paired-segment DAG failed, continuing"
            );
        } else {
            warn!(
                session_id = %session_id,
                "Paired-segment DAG failed for unknown session (init_session may be missing)"
            );
        }
    }

    /// Called when a per-segment DAG is about to start.
    pub fn on_dag_started(&self, session_id: &str, source: SourceType) {
        if let Some(mut session) = self.sessions.get_mut(session_id) {
            session.last_activity = Instant::now();
            match source {
                SourceType::Video => {
                    session.pending_video_dags = session.pending_video_dags.saturating_add(1);
                }
                SourceType::Danmu => {
                    session.danmu_observed = true;
                    session.pending_danmu_dags = session.pending_danmu_dags.saturating_add(1);
                }
            }
            debug!(
                session_id = %session_id,
                source = ?source,
                "DAG started, incremented pending count"
            );
        } else {
            warn!(
                session_id = %session_id,
                source = ?source,
                "DAG started for unknown session (init_session may be missing)"
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
        let outputs = dedup_paths_preserve_order(outputs);
        if let Some(mut session) = self.sessions.get_mut(session_id) {
            session.last_activity = Instant::now();
            // Decrement pending counter
            match source {
                SourceType::Video => {
                    if session.pending_video_dags == 0 {
                        warn!(
                            session_id = %session_id,
                            source = ?source,
                            segment_index = %segment_index,
                            "DAG completed but pending counter is already 0"
                        );
                    } else {
                        session.pending_video_dags -= 1;
                    }
                }
                SourceType::Danmu => {
                    session.danmu_observed = true;
                    if session.pending_danmu_dags == 0 {
                        warn!(
                            session_id = %session_id,
                            source = ?source,
                            segment_index = %segment_index,
                            "DAG completed but pending counter is already 0"
                        );
                    } else {
                        session.pending_danmu_dags -= 1;
                    }
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
        } else {
            warn!(
                session_id = %session_id,
                source = ?source,
                segment_index = %segment_index,
                "DAG completed for unknown session (init_session may be missing)"
            );
        }
    }

    /// Called when a per-segment DAG fails.
    pub fn on_dag_failed(&self, session_id: &str, source: SourceType) {
        if let Some(mut session) = self.sessions.get_mut(session_id) {
            session.last_activity = Instant::now();
            match source {
                SourceType::Video => {
                    if session.pending_video_dags == 0 {
                        warn!(
                            session_id = %session_id,
                            source = ?source,
                            "DAG failed but pending counter is already 0"
                        );
                    } else {
                        session.pending_video_dags -= 1;
                    }
                }
                SourceType::Danmu => {
                    session.danmu_observed = true;
                    if session.pending_danmu_dags == 0 {
                        warn!(
                            session_id = %session_id,
                            source = ?source,
                            "DAG failed but pending counter is already 0"
                        );
                    } else {
                        session.pending_danmu_dags -= 1;
                    }
                }
            }
            warn!(
                session_id = %session_id,
                source = ?source,
                "DAG failed, continuing with partial outputs"
            );
        } else {
            warn!(
                session_id = %session_id,
                source = ?source,
                "DAG failed for unknown session (init_session may be missing)"
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
            session.last_activity = Instant::now();
            let output = SegmentOutput {
                segment_index,
                path,
            };
            match source {
                SourceType::Video => session.video_outputs.push(output),
                SourceType::Danmu => {
                    session.danmu_observed = true;
                    session.danmu_outputs.push(output);
                }
            }
            debug!(
                session_id = %session_id,
                segment_index = %segment_index,
                source = ?source,
                "Raw segment collected (no DAG configured)"
            );
        } else {
            warn!(
                session_id = %session_id,
                segment_index = %segment_index,
                source = ?source,
                "Raw segment received for unknown session (init_session may be missing)"
            );
        }
    }

    /// Called when video stream completes (DownloadCompleted).
    pub fn on_video_complete(&self, session_id: &str) {
        if let Some(mut session) = self.sessions.get_mut(session_id) {
            session.last_activity = Instant::now();
            session.video_complete = true;
            info!(
                session_id = %session_id,
                "Video stream completed"
            );
        } else {
            warn!(
                session_id = %session_id,
                "Video stream completed for unknown session (init_session may be missing)"
            );
        }
    }

    /// Called when danmu collection stops (CollectionStopped).
    pub fn on_danmu_complete(&self, session_id: &str) {
        if let Some(mut session) = self.sessions.get_mut(session_id) {
            session.last_activity = Instant::now();
            session.danmu_observed = true;
            session.danmu_complete = true;
            info!(
                session_id = %session_id,
                "Danmu collection stopped"
            );
        } else {
            warn!(
                session_id = %session_id,
                "Danmu collection stopped for unknown session (init_session may be missing)"
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
            // Don't remove session state if outputs are still empty.
            // Completion events can be observed before the last SegmentCompleted/DAG completion is
            // fully processed; removing early would make the session pipeline untriggerable.
            if session.video_outputs.is_empty() && session.danmu_outputs.is_empty() {
                return None;
            }
        } else {
            return None;
        }

        // Remove and return the session
        if let Some((_, mut outputs)) = self.sessions.remove(session_id) {
            outputs.video_outputs.sort_by_key(|o| o.segment_index);
            outputs.danmu_outputs.sort_by_key(|o| o.segment_index);
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

    /// Check whether a session is ready to trigger and has at least one collected output.
    ///
    /// This is a non-consuming check intended for callers that need to apply additional
    /// gating (e.g., waiting for `end_time` in the sessions table) before calling
    /// `try_trigger()`.
    pub fn is_ready_nonempty(&self, session_id: &str) -> bool {
        let Some(session) = self.sessions.get(session_id) else {
            return false;
        };
        if !session.is_ready() {
            return false;
        }
        !(session.video_outputs.is_empty() && session.danmu_outputs.is_empty())
    }

    /// Cleanup stale sessions (TTL-based).
    pub fn cleanup_stale(&self, max_age_secs: u64) {
        let max_age = std::time::Duration::from_secs(max_age_secs);
        let mut removed = 0;

        self.sessions.retain(|session_id, session| {
            if session.last_activity.elapsed() > max_age {
                warn!(
                    session_id = %session_id,
                    age_secs = %session.last_activity.elapsed().as_secs(),
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
        coord.on_danmu_started("session1");

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

    #[test]
    fn test_early_complete_event_does_not_drop_session() {
        let coord = SessionCompleteCoordinator::new();

        coord.init_session("session1", "streamer1", false);

        coord.on_video_complete("session1");
        assert!(coord.try_trigger("session1").is_none());

        coord.on_raw_segment(
            "session1",
            0,
            PathBuf::from("/test/seg0.mp4"),
            SourceType::Video,
        );

        let outputs = coord
            .try_trigger("session1")
            .expect("Should be ready after output arrives");
        assert_eq!(outputs.video_outputs.len(), 1);
    }

    #[test]
    fn test_paired_dag_blocks_session_complete_until_done() {
        let coord = SessionCompleteCoordinator::new();

        coord.init_session("session1", "streamer1", false);
        coord.on_raw_segment("session1", 0, PathBuf::from("/seg0.mp4"), SourceType::Video);
        coord.on_video_complete("session1");

        // Ready immediately (no danmu expected/observed).
        assert!(coord.is_ready_nonempty("session1"));

        // Now a paired DAG starts; should block until it completes.
        coord.on_paired_dag_started("session1");
        assert!(!coord.is_ready_nonempty("session1"));
        assert!(coord.try_trigger("session1").is_none());

        coord.on_paired_dag_complete("session1");
        assert!(coord.is_ready_nonempty("session1"));
        assert!(coord.try_trigger("session1").is_some());
    }

    #[test]
    fn test_paired_segment_ready_once() {
        let coord = PairedSegmentCoordinator::new();

        assert!(
            coord
                .on_video_ready("session1", "streamer1", 0, vec![PathBuf::from("/seg0.mp4")])
                .is_none()
        );

        let ready = coord
            .on_danmu_ready(
                "session1",
                "streamer1",
                0,
                vec![PathBuf::from("/seg0.json")],
            )
            .expect("pair should become ready");

        assert_eq!(ready.segment_index, 0);
        assert_eq!(ready.video_outputs.len(), 1);
        assert_eq!(ready.danmu_outputs.len(), 1);

        // Should not trigger again for the same segment unless re-added.
        assert!(coord.active_pair_count() == 0);
    }
}
