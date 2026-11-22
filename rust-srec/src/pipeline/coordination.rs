//! Session coordination for collecting DAG outputs and triggering session-complete pipelines.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use tracing::{info, trace, warn};

use crate::database::models::DagSegmentSource;
use crate::database::models::job::DagPipelineDefinition;
use crate::pipeline::manifest::ManifestSegment;

/// Source type for segment outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceType {
    Video,
    Danmu,
}

impl SourceType {
    pub fn segment_source(self) -> DagSegmentSource {
        match self {
            Self::Video => DagSegmentSource::Video,
            Self::Danmu => DagSegmentSource::Danmu,
        }
    }
}

// Lives beside `SourceType` so both conversions are in one place without the
// database model depending on the pipeline.
impl DagSegmentSource {
    /// The per-source segment DAG kind, or `None` for paired and session-complete DAGs.
    pub fn source_type(self) -> Option<SourceType> {
        match self {
            Self::Video => Some(SourceType::Video),
            Self::Danmu => Some(SourceType::Danmu),
            Self::Paired | Self::SessionComplete => None,
        }
    }
}

pub(super) fn dedup_paths_preserve_order(paths: Vec<PathBuf>) -> Vec<PathBuf> {
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

impl PairedSegmentOutputs {
    /// The segment's video and danmu artifacts in collected order.
    pub fn manifest_segment(&self) -> ManifestSegment {
        ManifestSegment {
            segment_index: self.segment_index,
            video: paths_to_strings(&self.video_outputs),
            danmu: paths_to_strings(&self.danmu_outputs),
        }
    }
}

fn paths_to_strings(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

/// Snapshot of coordinator state at the moment `SessionPipelineState::try_finalize`
/// emits `CreateSessionCompleteDag`. It carries sorted artifact inputs together
/// with the readiness flags and monotonic observation times that produced the
/// dispatch. The runner uses the identities and paths to build the DAG's input
/// manifest.
#[derive(Debug, Clone)]
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
    /// Monotonic time when the coordinator first observed this session,
    /// distinct from the recording's persisted wall-clock start time.
    pub created_at: Instant,
    pub last_activity: Instant,

    /// Video segment outputs (from per-segment DAGs)
    pub video_outputs: Vec<SegmentOutput>,
    /// Danmu segment outputs (from per-segment DAGs)
    pub danmu_outputs: Vec<SegmentOutput>,

    /// Stream completion flags
    pub video_complete: bool,
    pub danmu_complete: bool,
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
        }
    }

    /// Get all outputs sorted by segment index.
    pub fn get_sorted_video_outputs(&self) -> Vec<PathBuf> {
        let mut outputs = self.video_outputs.to_vec();
        outputs.sort_by_key(|o| o.segment_index);
        outputs.into_iter().map(|o| o.path).collect()
    }

    /// Video and danmu outputs grouped by segment, ascending by segment index.
    /// A segment that produced only one of the two kinds is still listed so its
    /// videos are never paired with a neighbouring segment's danmu.
    pub fn manifest_segments(&self) -> Vec<ManifestSegment> {
        fn segment(
            segments: &mut BTreeMap<u32, ManifestSegment>,
            index: u32,
        ) -> &mut ManifestSegment {
            segments.entry(index).or_insert_with(|| ManifestSegment {
                segment_index: index,
                video: Vec::new(),
                danmu: Vec::new(),
            })
        }
        let mut segments = BTreeMap::new();
        for output in &self.video_outputs {
            segment(&mut segments, output.segment_index)
                .video
                .push(output.path.to_string_lossy().into_owned());
        }
        for output in &self.danmu_outputs {
            segment(&mut segments, output.segment_index)
                .danmu
                .push(output.path.to_string_lossy().into_owned());
        }
        segments.into_values().collect()
    }
}

#[derive(Debug, Clone)]
pub enum PipelineCoordinationEvent {
    ConfigureSession {
        session_id: String,
        streamer_id: String,
        danmu_enabled: bool,
        segment_pipeline: Option<DagPipelineDefinition>,
        paired_segment_pipeline: Option<DagPipelineDefinition>,
        session_complete_pipeline: Option<DagPipelineDefinition>,
    },
    VideoSegmentCompleted {
        session_id: String,
        streamer_id: String,
        segment_index: u32,
        path: PathBuf,
    },
    DanmuCollectionStarted {
        session_id: String,
        streamer_id: String,
    },
    DanmuCollectionStopped {
        session_id: String,
    },
    DanmuSegmentCompleted {
        session_id: String,
        streamer_id: String,
        segment_index: u32,
        path: PathBuf,
    },
    RecoverSourceArtifact {
        session_id: String,
        streamer_id: String,
        segment_index: u32,
        source: SourceType,
        path: PathBuf,
    },
    RecoverSegmentDagCompleted {
        session_id: String,
        streamer_id: String,
        segment_index: u32,
        source: SourceType,
        outputs: Vec<PathBuf>,
    },
    RecoverSegmentDagFailed {
        session_id: String,
        segment_index: u32,
        source: SourceType,
    },
    RecoverPairedDagTriggered {
        session_id: String,
        streamer_id: String,
        segment_index: u32,
    },
    SessionCompleteDagStarted {
        session_id: String,
        streamer_id: String,
    },
    SessionCompleteDagCreationFailed {
        session_id: String,
    },
    SegmentDagStarted {
        session_id: String,
        streamer_id: String,
        segment_index: u32,
        source: SourceType,
    },
    SegmentDagCompleted {
        session_id: String,
        streamer_id: String,
        segment_index: u32,
        source: SourceType,
        outputs: Vec<PathBuf>,
    },
    SegmentDagFailed {
        session_id: String,
        segment_index: u32,
        source: SourceType,
    },
    PairedDagStarted {
        session_id: String,
        streamer_id: String,
        segment_index: u32,
    },
    PairedDagCompleted {
        session_id: String,
    },
    PairedDagFailed {
        session_id: String,
    },
    SessionEnded {
        session_id: String,
        streamer_id: String,
        should_run_session_complete: bool,
    },
    SessionEndPersisted {
        session_id: String,
    },
}

#[derive(Debug, Clone)]
pub enum PipelineCommand {
    CreateSegmentDag {
        session_id: String,
        streamer_id: String,
        segment_index: u32,
        source: SourceType,
        input_path: PathBuf,
        pipeline: DagPipelineDefinition,
    },
    CreatePairedSegmentDag {
        outputs: PairedSegmentOutputs,
        pipeline: DagPipelineDefinition,
    },
    CreateSessionCompleteDag {
        outputs: SessionOutputs,
        pipeline: DagPipelineDefinition,
    },
}

/// Pipeline work `PipelineCoordinator` is still tracking for one session.
///
/// This is reducer state, not database rows, so it covers the windows a query
/// over `dag_execution` cannot see: a DAG whose row is already terminal but
/// whose `SegmentDagCompleted` has not been applied yet, and the whole span in
/// which a session-complete run is decided on and emitted but no row for it
/// exists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionCoordinationOutstanding {
    /// `SessionEndPersisted` reached the final-pipeline gate. End causes that
    /// skip this gate record their durable skip receipt directly.
    pub end_processed: bool,
    /// Segment, danmu and paired DAGs the reducer has counted as started and not
    /// yet seen complete, plus paired segments whose creation command has been
    /// emitted but whose `PairedDagStarted` has not been applied.
    pub pending_dags: u32,
    /// `SessionEnded` arrived with `should_run_session_complete`, a
    /// `CreateSessionCompleteDag` command can still be emitted for the session,
    /// and none has been settled yet - so a session-complete DAG is owed and
    /// `try_finalize` may emit its creation command at any point.
    ///
    /// Deliberately wider than `SessionPipelineState::pending_session_complete_start`,
    /// which only covers the span after `is_ready` holds.
    /// `PipelineManager::handle_session_transition` applies `ConfigureSession`,
    /// `SessionEnded` and `SessionEndPersisted` as three separate reducer calls
    /// with a `get_config_for_streamer` database read in front of them, and
    /// `is_ready` requires the last of the three, so a narrower flag would read
    /// as idle for the whole gap between them.
    ///
    /// It is also narrower than "the session ended": see
    /// `SessionPipelineState::session_complete_possible`, which drops the flag
    /// for sessions whose finalize gate can no longer open, and
    /// `session_complete_settled`, which drops it once no further command can
    /// come. Both matter because the only other bound is
    /// `PipelineCoordinatorState::cleanup_stale`, two days out.
    pub session_complete_owed: bool,
}

impl SessionCoordinationOutstanding {
    /// Whether the reducer is holding nothing further for the session.
    pub fn is_idle(&self) -> bool {
        self.pending_dags == 0 && !self.session_complete_owed
    }
}

/// Serializes reducer access behind one lock. The reducer is synchronous and
/// nonblocking, so callers apply events directly; never hold the guard across
/// an await.
#[derive(Debug, Clone)]
pub struct PipelineCoordinator {
    inner: Arc<Mutex<PipelineCoordinatorState>>,
}

impl PipelineCoordinator {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(PipelineCoordinatorState::new())),
        }
    }

    /// Apply an event and return the commands the caller must execute.
    pub fn apply_event(&self, event: PipelineCoordinationEvent) -> Vec<PipelineCommand> {
        self.inner.lock().apply_event(event)
    }

    pub fn cleanup_stale(&self, session_ttl_secs: u64) {
        self.inner.lock().cleanup_stale(session_ttl_secs);
    }

    #[cfg(test)]
    pub fn active_session_count(&self) -> usize {
        self.inner.lock().active_session_count()
    }

    #[cfg(test)]
    pub fn active_pair_count(&self) -> usize {
        self.inner.lock().active_pair_count()
    }

    /// What the reducer still holds for `session_id`, or `None` when it tracks
    /// no such session - either it never saw one or `cleanup_stale` dropped it.
    ///
    /// `PipelineManager::drain_for_session` pairs this with its `job` and
    /// `dag_execution` queries; neither source alone sees all in-flight work.
    pub fn session_outstanding(&self, session_id: &str) -> Option<SessionCoordinationOutstanding> {
        self.inner.lock().session_outstanding(session_id)
    }

    /// Whether a resolved streamer configuration has been applied to `session_id`.
    ///
    /// Callers send `ConfigureSession` only with a configuration they resolved,
    /// so a session whose lookups all failed stays unconfigured.
    pub fn has_configured_session(&self, session_id: &str) -> bool {
        self.inner
            .lock()
            .sessions
            .get(session_id)
            .is_some_and(|session| session.configured)
    }
}

impl Default for PipelineCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct PipelineCoordinatorState {
    sessions: HashMap<String, SessionPipelineState>,
}

impl PipelineCoordinatorState {
    fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    fn apply_event(&mut self, event: PipelineCoordinationEvent) -> Vec<PipelineCommand> {
        match event {
            PipelineCoordinationEvent::ConfigureSession {
                session_id,
                streamer_id,
                danmu_enabled,
                segment_pipeline,
                paired_segment_pipeline,
                session_complete_pipeline,
            } => {
                let session = self.session_mut(&session_id, &streamer_id);
                session.configure(
                    streamer_id,
                    danmu_enabled,
                    segment_pipeline,
                    paired_segment_pipeline,
                    session_complete_pipeline,
                );
                session.try_finalize()
            }
            PipelineCoordinationEvent::VideoSegmentCompleted {
                session_id,
                streamer_id,
                segment_index,
                path,
            } => {
                let session = self.session_mut(&session_id, &streamer_id);
                session.on_source_artifact(SourceType::Video, segment_index, path)
            }
            PipelineCoordinationEvent::DanmuCollectionStarted {
                session_id,
                streamer_id,
            } => {
                let session = self.session_mut(&session_id, &streamer_id);
                session.last_activity = Instant::now();
                session.danmu_expected = true;
                session.danmu_observed = true;
                Vec::new()
            }
            PipelineCoordinationEvent::DanmuCollectionStopped { session_id } => {
                let Some(session) = self.sessions.get_mut(&session_id) else {
                    trace!(session_id = %session_id, "Danmu stopped for unknown coordinator session");
                    return Vec::new();
                };
                session.last_activity = Instant::now();
                session.danmu_observed = true;
                session.danmu_complete = true;
                session.try_finalize()
            }
            PipelineCoordinationEvent::DanmuSegmentCompleted {
                session_id,
                streamer_id,
                segment_index,
                path,
            } => {
                let session = self.session_mut(&session_id, &streamer_id);
                session.danmu_expected = true;
                session.danmu_observed = true;
                session.on_source_artifact(SourceType::Danmu, segment_index, path)
            }
            PipelineCoordinationEvent::RecoverSourceArtifact {
                session_id,
                streamer_id,
                segment_index,
                source,
                path,
            } => {
                let session = self.session_mut(&session_id, &streamer_id);
                session.recover_source_artifact(source, segment_index, path)
            }
            PipelineCoordinationEvent::RecoverSegmentDagCompleted {
                session_id,
                streamer_id,
                segment_index,
                source,
                outputs,
            } => {
                let session = self.session_mut(&session_id, &streamer_id);
                session.recover_segment_dag_completed(source, segment_index, outputs)
            }
            PipelineCoordinationEvent::RecoverSegmentDagFailed {
                session_id,
                segment_index,
                source,
            } => {
                let Some(session) = self.sessions.get_mut(&session_id) else {
                    trace!(
                        session_id = %session_id,
                        source = ?source,
                        "Recovered segment DAG failure for unknown coordinator session"
                    );
                    return Vec::new();
                };
                session.recover_segment_dag_failed(source, segment_index)
            }
            PipelineCoordinationEvent::RecoverPairedDagTriggered {
                session_id,
                streamer_id,
                segment_index,
            } => {
                let session = self.session_mut(&session_id, &streamer_id);
                session.recover_paired_dag_triggered(segment_index);
                Vec::new()
            }
            PipelineCoordinationEvent::SessionCompleteDagStarted {
                session_id,
                streamer_id,
            } => {
                let session = self.session_mut(&session_id, &streamer_id);
                session.pending_session_complete_start = false;
                session.session_complete_triggered = true;
                session.session_complete_settled = true;
                Vec::new()
            }
            PipelineCoordinationEvent::SessionCompleteDagCreationFailed { session_id } => {
                let Some(session) = self.sessions.get_mut(&session_id) else {
                    return Vec::new();
                };
                session.pending_session_complete_start = false;
                // Publication failed, so the required dispatch is still owed.
                // Recovery can retry it while its streamer/configuration remains.
                session.session_complete_settled = false;
                Vec::new()
            }
            PipelineCoordinationEvent::SegmentDagStarted {
                session_id,
                streamer_id,
                segment_index,
                source,
            } => {
                let session = self.session_mut(&session_id, &streamer_id);
                session.on_segment_dag_started(source, segment_index);
                Vec::new()
            }
            PipelineCoordinationEvent::SegmentDagCompleted {
                session_id,
                streamer_id,
                segment_index,
                source,
                outputs,
            } => {
                let session = self.session_mut(&session_id, &streamer_id);
                session.on_segment_dag_completed(source, segment_index, outputs)
            }
            PipelineCoordinationEvent::SegmentDagFailed {
                session_id,
                segment_index,
                source,
            } => {
                let Some(session) = self.sessions.get_mut(&session_id) else {
                    trace!(
                        session_id = %session_id,
                        source = ?source,
                        "Segment DAG failed for unknown coordinator session"
                    );
                    return Vec::new();
                };
                session.on_segment_dag_failed(source, segment_index)
            }
            PipelineCoordinationEvent::PairedDagStarted {
                session_id,
                streamer_id,
                segment_index,
            } => {
                let session = self.session_mut(&session_id, &streamer_id);
                session.on_paired_dag_started(segment_index);
                Vec::new()
            }
            PipelineCoordinationEvent::PairedDagCompleted { session_id } => {
                let Some(session) = self.sessions.get_mut(&session_id) else {
                    trace!(session_id = %session_id, "Paired DAG completed for unknown coordinator session");
                    return Vec::new();
                };
                session.on_paired_dag_finished(true)
            }
            PipelineCoordinationEvent::PairedDagFailed { session_id } => {
                let Some(session) = self.sessions.get_mut(&session_id) else {
                    trace!(session_id = %session_id, "Paired DAG failed for unknown coordinator session");
                    return Vec::new();
                };
                session.on_paired_dag_finished(false)
            }
            PipelineCoordinationEvent::SessionEnded {
                session_id,
                streamer_id,
                should_run_session_complete,
            } => {
                let session = self.session_mut(&session_id, &streamer_id);
                session.last_activity = Instant::now();
                session.session_end_observed = should_run_session_complete;
                session.session_end_persisted = false;
                session.video_complete = should_run_session_complete;
                session.try_finalize()
            }
            PipelineCoordinationEvent::SessionEndPersisted { session_id } => {
                let Some(session) = self.sessions.get_mut(&session_id) else {
                    trace!(session_id = %session_id, "Session end persisted for unknown coordinator session");
                    return Vec::new();
                };
                session.last_activity = Instant::now();
                session.session_end_persisted = true;
                session.try_finalize()
            }
        }
    }

    fn session_mut(&mut self, session_id: &str, streamer_id: &str) -> &mut SessionPipelineState {
        self.sessions
            .entry(session_id.to_string())
            .and_modify(|session| {
                session.last_activity = Instant::now();
                if session.streamer_id != streamer_id {
                    warn!(
                        session_id = %session_id,
                        existing_streamer_id = %session.streamer_id,
                        new_streamer_id = %streamer_id,
                        "Pipeline coordinator streamer_id mismatch (keeping existing)"
                    );
                }
            })
            .or_insert_with(|| {
                SessionPipelineState::new(session_id.to_string(), streamer_id.to_string())
            })
    }

    fn session_outstanding(&self, session_id: &str) -> Option<SessionCoordinationOutstanding> {
        self.sessions
            .get(session_id)
            .map(SessionPipelineState::outstanding)
    }

    #[cfg(test)]
    fn active_session_count(&self) -> usize {
        self.sessions.len()
    }

    #[cfg(test)]
    fn active_pair_count(&self) -> usize {
        self.sessions
            .values()
            .map(SessionPipelineState::active_pair_count)
            .sum()
    }

    fn cleanup_stale(&mut self, session_ttl_secs: u64) {
        let max_age = std::time::Duration::from_secs(session_ttl_secs);
        let mut removed = 0;
        self.sessions.retain(|session_id, session| {
            if session.last_activity.elapsed() > max_age {
                // `unmet` is the diagnosis: discarding a session that never
                // triggered means its session-complete pipeline is lost, and the
                // gate condition still outstanding says which producer went
                // missing.
                warn!(
                    session_id = %session_id,
                    age_secs = %session.last_activity.elapsed().as_secs(),
                    session_complete_triggered = session.session_complete_triggered,
                    pending_session_complete_start = session.pending_session_complete_start,
                    unmet = ?session.unmet_completion_conditions(),
                    "Removing stale pipeline coordinator session"
                );
                removed += 1;
                false
            } else {
                true
            }
        });

        if removed > 0 {
            info!(removed = %removed, "Cleaned up stale pipeline coordinator sessions");
        }
    }
}

mod session_state;
#[cfg(test)]
mod tests;

pub use session_state::SessionPipelineState;
