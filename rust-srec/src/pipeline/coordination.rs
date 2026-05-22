//! Session coordination for collecting DAG outputs and triggering session-complete pipelines.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{info, trace, warn};

use crate::database::models::job::DagPipelineDefinition;

/// Source type for segment outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceType {
    Video,
    Danmu,
}

impl SourceType {
    pub fn as_segment_source(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Danmu => "danmu",
        }
    }
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

/// Snapshot of coordinator state at the moment `SessionPipelineState::try_finalize`
/// emits `CreateSessionCompleteDag`. Read today by `run_session_complete_pipeline`
/// (which uses only `session_id`, `streamer_id`, `video_outputs`, `danmu_outputs`
/// to build the on-disk `session_<id>_inputs.json` manifest); the remaining
/// fields are kept as the natural source for a planned `session_pipeline_runs`
/// DB row that will retire the JSON manifest.
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
    /// When `PipelineCoordinator` first saw this session (distinct from
    /// `live_sessions.created_at`, which is when `SessionLifecycle` inserted
    /// the row).
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

    /// Get all danmu outputs sorted by segment index.
    pub fn get_sorted_danmu_outputs(&self) -> Vec<PathBuf> {
        let mut outputs = self.danmu_outputs.to_vec();
        outputs.sort_by_key(|o| o.segment_index);
        outputs.into_iter().map(|o| o.path).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub enum PipelineScope {
    Segment {
        source: SourceType,
        segment_index: u32,
    },
    PairedSegment {
        segment_index: u32,
    },
    SessionComplete,
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

#[derive(Debug)]
enum CoordinatorRequest {
    Apply {
        event: PipelineCoordinationEvent,
        reply: oneshot::Sender<Vec<PipelineCommand>>,
    },
    Cleanup {
        session_ttl_secs: u64,
    },
    ActiveSessionCount {
        reply: oneshot::Sender<usize>,
    },
    ActivePairCount {
        reply: oneshot::Sender<usize>,
    },
}

#[derive(Debug, Clone)]
pub struct PipelineCoordinator {
    inner: Arc<Mutex<PipelineCoordinatorState>>,
    tx: Arc<AsyncMutex<Option<mpsc::Sender<CoordinatorRequest>>>>,
}

impl PipelineCoordinator {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(PipelineCoordinatorState::new())),
            tx: Arc::new(AsyncMutex::new(None)),
        }
    }

    pub async fn start(&self, cancellation_token: CancellationToken) {
        let mut tx_guard = self.tx.lock().await;
        if tx_guard.is_some() {
            return;
        }

        let (tx, mut rx) = mpsc::channel::<CoordinatorRequest>(1024);
        let inner = self.inner.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => break,
                    request = rx.recv() => {
                        let Some(request) = request else {
                            break;
                        };
                        Self::handle_request(&inner, request);
                    }
                }
            }
        });
        *tx_guard = Some(tx);
    }

    fn lock_state(
        inner: &Arc<Mutex<PipelineCoordinatorState>>,
    ) -> std::sync::MutexGuard<'_, PipelineCoordinatorState> {
        match inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                warn!("Pipeline coordinator state lock was poisoned; recovering state");
                poisoned.into_inner()
            }
        }
    }

    fn handle_request(inner: &Arc<Mutex<PipelineCoordinatorState>>, request: CoordinatorRequest) {
        match request {
            CoordinatorRequest::Apply { event, reply } => {
                let commands = Self::lock_state(inner).apply_event(event);
                let _ = reply.send(commands);
            }
            CoordinatorRequest::Cleanup { session_ttl_secs } => {
                Self::lock_state(inner).cleanup_stale(session_ttl_secs);
            }
            CoordinatorRequest::ActiveSessionCount { reply } => {
                let count = Self::lock_state(inner).active_session_count();
                let _ = reply.send(count);
            }
            CoordinatorRequest::ActivePairCount { reply } => {
                let count = Self::lock_state(inner).active_pair_count();
                let _ = reply.send(count);
            }
        }
    }

    /// Apply an event through the bounded coordinator actor when it is running.
    ///
    /// Production code should use this method consistently. `apply_event_inline`
    /// is for tests and pre-actor bootstrap paths only; do not mix the two for
    /// the same live coordinator after `start` has been called.
    pub async fn apply_event(&self, event: PipelineCoordinationEvent) -> Vec<PipelineCommand> {
        let tx = { self.tx.lock().await.clone() };
        let Some(tx) = tx else {
            return self.apply_event_inline(event);
        };

        let (reply, rx) = oneshot::channel();
        if tx
            .send(CoordinatorRequest::Apply { event, reply })
            .await
            .is_err()
        {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    /// Apply an event by taking the coordinator state lock directly.
    ///
    /// This is intended for reducer tests and bootstrap use before the actor is
    /// started. Runtime code should use `apply_event` so events stay serialized
    /// through the bounded channel.
    pub fn apply_event_inline(&self, event: PipelineCoordinationEvent) -> Vec<PipelineCommand> {
        Self::lock_state(&self.inner).apply_event(event)
    }

    pub async fn cleanup_stale(&self, session_ttl_secs: u64) {
        let tx = { self.tx.lock().await.clone() };
        if let Some(tx) = tx {
            let _ = tx
                .send(CoordinatorRequest::Cleanup { session_ttl_secs })
                .await;
        } else {
            Self::lock_state(&self.inner).cleanup_stale(session_ttl_secs);
        }
    }

    pub async fn active_session_count(&self) -> usize {
        let tx = { self.tx.lock().await.clone() };
        let Some(tx) = tx else {
            return self.active_session_count_inline();
        };

        let (reply, rx) = oneshot::channel();
        if tx
            .send(CoordinatorRequest::ActiveSessionCount { reply })
            .await
            .is_err()
        {
            return 0;
        }
        rx.await.unwrap_or(0)
    }

    pub fn active_session_count_inline(&self) -> usize {
        Self::lock_state(&self.inner).active_session_count()
    }

    pub async fn active_pair_count(&self) -> usize {
        let tx = { self.tx.lock().await.clone() };
        let Some(tx) = tx else {
            return self.active_pair_count_inline();
        };

        let (reply, rx) = oneshot::channel();
        if tx
            .send(CoordinatorRequest::ActivePairCount { reply })
            .await
            .is_err()
        {
            return 0;
        }
        rx.await.unwrap_or(0)
    }

    pub fn active_pair_count_inline(&self) -> usize {
        Self::lock_state(&self.inner).active_pair_count()
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

    fn active_session_count(&self) -> usize {
        self.sessions.len()
    }

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
                warn!(
                    session_id = %session_id,
                    age_secs = %session.last_activity.elapsed().as_secs(),
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

#[derive(Debug)]
pub struct SessionPipelineState {
    session_id: String,
    streamer_id: String,
    danmu_expected: bool,
    danmu_observed: bool,
    video_complete: bool,
    danmu_complete: bool,
    session_end_observed: bool,
    session_end_persisted: bool,
    session_complete_triggered: bool,
    /// When `PipelineCoordinator` first observed this session (via any reducer
    /// event). Distinct from `live_sessions.created_at`, which is the DB row
    /// insertion time written by `SessionLifecycle`. Copied into
    /// `SessionOutputs::created_at` at session-complete fire time so the
    /// planned `session_pipeline_runs` DB row can record post-processing
    /// coordination latency.
    created_at: Instant,
    last_activity: Instant,
    segment_pipeline: Option<DagPipelineDefinition>,
    paired_segment_pipeline: Option<DagPipelineDefinition>,
    session_complete_pipeline: Option<DagPipelineDefinition>,
    segments: BTreeMap<u32, SegmentState>,
    pending_video_dags: u32,
    pending_danmu_dags: u32,
    pending_paired_dags: u32,
    started_segment_dags: HashSet<(SourceType, u32)>,
    started_paired_dags: HashSet<u32>,
    triggered_paired_segments: HashSet<u32>,
    /// Segment indices for which `try_trigger_paired` emitted a
    /// `CreatePairedSegmentDag` command but `on_paired_dag_started` has not
    /// yet been applied. Without this set, `try_finalize` can fire
    /// session-complete in the same reducer call that just emitted the
    /// paired command, because `pending_paired_dags` only counts started
    /// DAGs. The gate in `artifacts_drained_for_session_complete` rejects
    /// finalization while this set is non-empty.
    pending_paired_starts: HashSet<u32>,
}

impl SessionPipelineState {
    fn new(session_id: String, streamer_id: String) -> Self {
        let now = Instant::now();
        Self {
            session_id,
            streamer_id,
            danmu_expected: false,
            danmu_observed: false,
            video_complete: false,
            danmu_complete: false,
            session_end_observed: false,
            session_end_persisted: false,
            session_complete_triggered: false,
            created_at: now,
            last_activity: now,
            segment_pipeline: None,
            paired_segment_pipeline: None,
            session_complete_pipeline: None,
            segments: BTreeMap::new(),
            pending_video_dags: 0,
            pending_danmu_dags: 0,
            pending_paired_dags: 0,
            started_segment_dags: HashSet::new(),
            started_paired_dags: HashSet::new(),
            triggered_paired_segments: HashSet::new(),
            pending_paired_starts: HashSet::new(),
        }
    }

    fn configure(
        &mut self,
        streamer_id: String,
        danmu_enabled: bool,
        segment_pipeline: Option<DagPipelineDefinition>,
        paired_segment_pipeline: Option<DagPipelineDefinition>,
        session_complete_pipeline: Option<DagPipelineDefinition>,
    ) {
        self.last_activity = Instant::now();
        if self.streamer_id != streamer_id {
            warn!(
                session_id = %self.session_id,
                existing_streamer_id = %self.streamer_id,
                new_streamer_id = %streamer_id,
                "Pipeline coordinator configure streamer_id mismatch (keeping existing)"
            );
        }
        self.danmu_expected |= danmu_enabled;
        if segment_pipeline.is_some() {
            self.segment_pipeline = segment_pipeline;
        }
        if paired_segment_pipeline.is_some() {
            self.paired_segment_pipeline = paired_segment_pipeline;
        }
        if session_complete_pipeline.is_some() {
            self.session_complete_pipeline = session_complete_pipeline;
        }
    }

    fn segment_mut(&mut self, segment_index: u32) -> &mut SegmentState {
        self.segments.entry(segment_index).or_default()
    }

    fn on_source_artifact(
        &mut self,
        source: SourceType,
        segment_index: u32,
        path: PathBuf,
    ) -> Vec<PipelineCommand> {
        self.last_activity = Instant::now();
        let mut commands = Vec::new();
        let has_segment_pipeline = self
            .segment_pipeline
            .as_ref()
            .is_some_and(|pipeline| !pipeline.is_empty());

        if source == SourceType::Danmu {
            self.danmu_observed = true;
        }

        if has_segment_pipeline {
            let should_start_dag = {
                let artifact = self.segment_mut(segment_index).artifact_mut(source);
                artifact.add_source(path.clone());
                if artifact.dag_started {
                    false
                } else {
                    artifact.dag_started = true;
                    true
                }
            };
            if should_start_dag && let Some(pipeline) = self.segment_pipeline.clone() {
                commands.push(PipelineCommand::CreateSegmentDag {
                    session_id: self.session_id.clone(),
                    streamer_id: self.streamer_id.clone(),
                    segment_index,
                    source,
                    input_path: path,
                    pipeline,
                });
            }
        } else {
            self.segment_mut(segment_index)
                .artifact_mut(source)
                .add_final(path);
            commands.extend(self.try_trigger_paired(segment_index));
            commands.extend(self.try_finalize());
        }

        commands
    }

    fn recover_source_artifact(
        &mut self,
        source: SourceType,
        segment_index: u32,
        path: PathBuf,
    ) -> Vec<PipelineCommand> {
        self.last_activity = Instant::now();
        if source == SourceType::Danmu {
            self.danmu_observed = true;
        }

        let has_segment_pipeline = self
            .segment_pipeline
            .as_ref()
            .is_some_and(|pipeline| !pipeline.is_empty());
        let artifact = self.segment_mut(segment_index).artifact_mut(source);
        artifact.add_source(path.clone());

        if has_segment_pipeline {
            return Vec::new();
        }

        artifact.add_final(path);
        let mut commands = self.try_trigger_paired(segment_index);
        commands.extend(self.try_finalize());
        commands
    }

    fn recover_segment_dag_completed(
        &mut self,
        source: SourceType,
        segment_index: u32,
        outputs: Vec<PathBuf>,
    ) -> Vec<PipelineCommand> {
        self.last_activity = Instant::now();
        if source == SourceType::Danmu {
            self.danmu_observed = true;
        }

        let outputs = dedup_paths_preserve_order(outputs);
        let artifact = self.segment_mut(segment_index).artifact_mut(source);
        artifact.dag_started = true;
        for output in outputs {
            artifact.add_final(output);
        }

        let mut commands = self.try_trigger_paired(segment_index);
        commands.extend(self.try_finalize());
        commands
    }

    fn recover_segment_dag_failed(
        &mut self,
        source: SourceType,
        segment_index: u32,
    ) -> Vec<PipelineCommand> {
        self.last_activity = Instant::now();
        if source == SourceType::Danmu {
            self.danmu_observed = true;
        }

        if let Some(segment) = self.segments.get_mut(&segment_index) {
            let artifact = segment.artifact_mut(source);
            artifact.dag_started = true;
            artifact.use_source_inputs_as_failed_fallback();
        } else {
            self.segment_mut(segment_index)
                .artifact_mut(source)
                .dag_started = true;
        }

        let ready_segments: Vec<u32> = self
            .segments
            .iter()
            .filter_map(|(idx, segment)| {
                (!segment.video.final_outputs.is_empty() && !segment.danmu.final_outputs.is_empty())
                    .then_some(*idx)
            })
            .collect();
        let mut commands = Vec::new();
        for segment_index in ready_segments {
            commands.extend(self.try_trigger_paired(segment_index));
        }
        commands.extend(self.try_finalize());
        commands
    }

    fn on_segment_dag_started(&mut self, source: SourceType, segment_index: u32) {
        self.last_activity = Instant::now();
        if !self.started_segment_dags.insert((source, segment_index)) {
            trace!(
                session_id = %self.session_id,
                segment_index = %segment_index,
                source = ?source,
                "Ignoring duplicate segment DAG start"
            );
            return;
        }

        match source {
            SourceType::Video => {
                self.pending_video_dags = self.pending_video_dags.saturating_add(1);
            }
            SourceType::Danmu => {
                self.danmu_observed = true;
                self.pending_danmu_dags = self.pending_danmu_dags.saturating_add(1);
            }
        }
        self.segment_mut(segment_index)
            .artifact_mut(source)
            .dag_started = true;
    }

    fn on_segment_dag_completed(
        &mut self,
        source: SourceType,
        segment_index: u32,
        outputs: Vec<PathBuf>,
    ) -> Vec<PipelineCommand> {
        self.last_activity = Instant::now();
        self.decrement_segment_pending(source, segment_index, "completed");

        let outputs = dedup_paths_preserve_order(outputs);
        let segment = self.segment_mut(segment_index);
        let artifact = segment.artifact_mut(source);
        for output in outputs {
            artifact.add_final(output);
        }

        let mut commands = self.try_trigger_paired(segment_index);
        commands.extend(self.try_finalize());
        commands
    }

    fn on_segment_dag_failed(
        &mut self,
        source: SourceType,
        segment_index: u32,
    ) -> Vec<PipelineCommand> {
        self.last_activity = Instant::now();
        self.decrement_segment_pending(source, segment_index, "failed");

        if let Some(segment) = self.segments.get_mut(&segment_index) {
            let artifact = segment.artifact_mut(source);
            artifact.use_source_inputs_as_failed_fallback();
        }

        let ready_segments: Vec<u32> = self
            .segments
            .iter()
            .filter_map(|(idx, segment)| {
                (!segment.video.final_outputs.is_empty() && !segment.danmu.final_outputs.is_empty())
                    .then_some(*idx)
            })
            .collect();
        let mut commands = Vec::new();
        for segment_index in ready_segments {
            commands.extend(self.try_trigger_paired(segment_index));
        }
        commands.extend(self.try_finalize());
        commands
    }

    fn decrement_segment_pending(&mut self, source: SourceType, segment_index: u32, reason: &str) {
        self.started_segment_dags.remove(&(source, segment_index));
        match source {
            SourceType::Video => {
                if self.pending_video_dags == 0 {
                    warn!(
                        session_id = %self.session_id,
                        segment_index = %segment_index,
                        source = ?source,
                        reason = %reason,
                        "Segment DAG finished but pending counter is already 0"
                    );
                } else {
                    self.pending_video_dags -= 1;
                }
            }
            SourceType::Danmu => {
                self.danmu_observed = true;
                if self.pending_danmu_dags == 0 {
                    warn!(
                        session_id = %self.session_id,
                        segment_index = %segment_index,
                        source = ?source,
                        reason = %reason,
                        "Segment DAG finished but pending counter is already 0"
                    );
                } else {
                    self.pending_danmu_dags -= 1;
                }
            }
        }
    }

    fn try_trigger_paired(&mut self, segment_index: u32) -> Vec<PipelineCommand> {
        if self.triggered_paired_segments.contains(&segment_index) {
            return Vec::new();
        }

        let Some(pipeline) = self.paired_segment_pipeline.clone() else {
            return Vec::new();
        };
        if pipeline.is_empty() {
            return Vec::new();
        }

        let Some(segment) = self.segments.get(&segment_index) else {
            return Vec::new();
        };
        let video_outputs = segment.video.final_outputs.clone();
        let danmu_outputs = segment.danmu.final_outputs.clone();
        if video_outputs.is_empty() || danmu_outputs.is_empty() {
            return Vec::new();
        }

        self.triggered_paired_segments.insert(segment_index);
        self.pending_paired_starts.insert(segment_index);
        vec![PipelineCommand::CreatePairedSegmentDag {
            outputs: PairedSegmentOutputs {
                session_id: self.session_id.clone(),
                streamer_id: self.streamer_id.clone(),
                segment_index,
                video_outputs,
                danmu_outputs,
            },
            pipeline,
        }]
    }

    fn on_paired_dag_started(&mut self, segment_index: u32) {
        self.last_activity = Instant::now();
        self.triggered_paired_segments.insert(segment_index);
        // `pending_paired_starts` is the gate that keeps `try_finalize`
        // from firing session-complete in the same reducer call that
        // emitted the paired command. Removing an absent index is a
        // no-op, so recovery-driven `PairedDagStarted` events that have
        // no prior `try_trigger_paired` in this process don't desync the
        // gate.
        self.pending_paired_starts.remove(&segment_index);
        if !self.started_paired_dags.insert(segment_index) {
            trace!(
                session_id = %self.session_id,
                segment_index = %segment_index,
                "Ignoring duplicate paired DAG start"
            );
            return;
        }
        self.pending_paired_dags = self.pending_paired_dags.saturating_add(1);
    }

    fn recover_paired_dag_triggered(&mut self, segment_index: u32) {
        self.last_activity = Instant::now();
        self.triggered_paired_segments.insert(segment_index);
    }

    fn on_paired_dag_finished(&mut self, succeeded: bool) -> Vec<PipelineCommand> {
        self.last_activity = Instant::now();
        if self.pending_paired_dags == 0 {
            warn!(
                session_id = %self.session_id,
                succeeded = %succeeded,
                "Paired DAG finished but pending counter is already 0"
            );
        } else {
            self.pending_paired_dags -= 1;
        }
        self.try_finalize()
    }

    fn try_finalize(&mut self) -> Vec<PipelineCommand> {
        if !self.is_ready() {
            self.log_not_ready();
            return Vec::new();
        }

        let Some(pipeline) = self.session_complete_pipeline.clone() else {
            warn!(
                session_id = %self.session_id,
                "Session became ready but no session_complete_pipeline definition was captured"
            );
            self.session_complete_triggered = true;
            return Vec::new();
        };

        self.session_complete_triggered = true;
        let outputs = self.session_outputs();
        info!(
            session_id = %self.session_id,
            video_outputs = %outputs.video_outputs.len(),
            danmu_outputs = %outputs.danmu_outputs.len(),
            "Session ready for session-complete pipeline"
        );
        vec![PipelineCommand::CreateSessionCompleteDag { outputs, pipeline }]
    }

    fn is_ready(&self) -> bool {
        self.session_end_observed
            && self.session_end_persisted
            && !self.session_complete_triggered
            && self.artifacts_drained_for_session_complete()
    }

    fn artifacts_drained_for_session_complete(&self) -> bool {
        let danmu_required = self.danmu_expected && self.danmu_observed;
        self.video_complete
            && self.pending_video_dags == 0
            && (!danmu_required || (self.danmu_complete && self.pending_danmu_dags == 0))
            && self.pending_paired_dags == 0
            && self.pending_paired_starts.is_empty()
            && self.has_video_output()
    }

    fn has_video_output(&self) -> bool {
        self.segments
            .values()
            .any(|segment| !segment.video.final_outputs.is_empty())
    }

    fn session_outputs(&self) -> SessionOutputs {
        let mut outputs = SessionOutputs::new(
            self.session_id.clone(),
            self.streamer_id.clone(),
            self.danmu_expected,
        );
        outputs.danmu_observed = self.danmu_observed;
        outputs.created_at = self.created_at;
        outputs.last_activity = self.last_activity;
        outputs.video_complete = self.video_complete;
        outputs.danmu_complete = self.danmu_complete;

        for (segment_index, segment) in &self.segments {
            for path in &segment.video.final_outputs {
                outputs.video_outputs.push(SegmentOutput {
                    segment_index: *segment_index,
                    path: path.clone(),
                });
            }
            for path in &segment.danmu.final_outputs {
                outputs.danmu_outputs.push(SegmentOutput {
                    segment_index: *segment_index,
                    path: path.clone(),
                });
            }
        }
        outputs
    }

    fn active_pair_count(&self) -> usize {
        self.segments
            .iter()
            .filter(|(idx, segment)| {
                !self.triggered_paired_segments.contains(idx)
                    && (!segment.video.final_outputs.is_empty()
                        || !segment.danmu.final_outputs.is_empty())
                    && self
                        .paired_segment_pipeline
                        .as_ref()
                        .is_some_and(|pipeline| !pipeline.is_empty())
            })
            .count()
    }

    fn log_not_ready(&self) {
        trace!(
            session_id = %self.session_id,
            streamer_id = %self.streamer_id,
            session_end_observed = %self.session_end_observed,
            session_end_persisted = %self.session_end_persisted,
            video_complete = %self.video_complete,
            danmu_expected = %self.danmu_expected,
            danmu_observed = %self.danmu_observed,
            danmu_complete = %self.danmu_complete,
            pending_video_dags = %self.pending_video_dags,
            pending_danmu_dags = %self.pending_danmu_dags,
            pending_paired_dags = %self.pending_paired_dags,
            pending_paired_starts = ?self.pending_paired_starts,
            has_video_output = %self.has_video_output(),
            session_complete_triggered = %self.session_complete_triggered,
            "Session not ready for session-complete trigger"
        );
    }
}

#[derive(Debug, Default)]
pub struct SegmentState {
    video: ArtifactLane,
    danmu: ArtifactLane,
}

impl SegmentState {
    fn artifact_mut(&mut self, source: SourceType) -> &mut ArtifactLane {
        match source {
            SourceType::Video => &mut self.video,
            SourceType::Danmu => &mut self.danmu,
        }
    }
}

#[derive(Debug, Default)]
pub struct ArtifactLane {
    source_inputs: Vec<PathBuf>,
    final_outputs: Vec<PathBuf>,
    dag_started: bool,
    final_outputs_are_fallback: bool,
}

impl ArtifactLane {
    fn add_source(&mut self, path: PathBuf) {
        self.source_inputs = dedup_paths_preserve_order(
            self.source_inputs
                .iter()
                .cloned()
                .chain(std::iter::once(path))
                .collect(),
        );
    }

    fn add_final(&mut self, path: PathBuf) {
        if self.final_outputs_are_fallback {
            self.final_outputs.clear();
            self.final_outputs_are_fallback = false;
        }
        self.final_outputs = dedup_paths_preserve_order(
            self.final_outputs
                .iter()
                .cloned()
                .chain(std::iter::once(path))
                .collect(),
        );
    }

    fn use_source_inputs_as_failed_fallback(&mut self) {
        if self.source_inputs.is_empty() {
            return;
        }
        self.final_outputs = self.source_inputs.clone();
        self.final_outputs_are_fallback = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::models::job::{DagStep, PipelineStep};

    fn empty_session_pipeline() -> DagPipelineDefinition {
        DagPipelineDefinition::new("session-complete", vec![])
    }

    fn non_empty_pipeline(name: &str) -> DagPipelineDefinition {
        DagPipelineDefinition::new(name, vec![DagStep::new("A", PipelineStep::preset("remux"))])
    }

    fn configure_event(
        session_id: &str,
        streamer_id: &str,
        danmu_enabled: bool,
        segment_pipeline: Option<DagPipelineDefinition>,
        paired_segment_pipeline: Option<DagPipelineDefinition>,
        session_complete_pipeline: Option<DagPipelineDefinition>,
    ) -> PipelineCoordinationEvent {
        PipelineCoordinationEvent::ConfigureSession {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            danmu_enabled,
            segment_pipeline,
            paired_segment_pipeline,
            session_complete_pipeline,
        }
    }

    fn end_event(session_id: &str, streamer_id: &str) -> PipelineCoordinationEvent {
        PipelineCoordinationEvent::SessionEnded {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            should_run_session_complete: true,
        }
    }

    fn persisted_event(session_id: &str) -> PipelineCoordinationEvent {
        PipelineCoordinationEvent::SessionEndPersisted {
            session_id: session_id.to_string(),
        }
    }

    #[test]
    fn pipeline_coordinator_waits_when_danmu_finishes_before_video() {
        let coord = PipelineCoordinator::new();
        let session_id = "session1";
        let streamer_id = "streamer1";

        assert!(
            coord
                .apply_event_inline(configure_event(
                    session_id,
                    streamer_id,
                    true,
                    None,
                    None,
                    Some(empty_session_pipeline()),
                ))
                .is_empty()
        );
        assert!(
            coord
                .apply_event_inline(PipelineCoordinationEvent::DanmuCollectionStarted {
                    session_id: session_id.to_string(),
                    streamer_id: streamer_id.to_string(),
                })
                .is_empty()
        );
        assert!(
            coord
                .apply_event_inline(PipelineCoordinationEvent::DanmuSegmentCompleted {
                    session_id: session_id.to_string(),
                    streamer_id: streamer_id.to_string(),
                    segment_index: 0,
                    path: PathBuf::from("/seg0.xml"),
                })
                .is_empty()
        );
        assert!(
            coord
                .apply_event_inline(PipelineCoordinationEvent::DanmuCollectionStopped {
                    session_id: session_id.to_string(),
                })
                .is_empty()
        );
        assert!(
            coord
                .apply_event_inline(end_event(session_id, streamer_id))
                .is_empty()
        );
        assert!(
            coord
                .apply_event_inline(persisted_event(session_id))
                .is_empty()
        );

        let commands = coord.apply_event_inline(PipelineCoordinationEvent::VideoSegmentCompleted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            segment_index: 0,
            path: PathBuf::from("/seg0.flv"),
        });

        assert_eq!(commands.len(), 1);
        match &commands[0] {
            PipelineCommand::CreateSessionCompleteDag { outputs, .. } => {
                assert_eq!(outputs.video_outputs.len(), 1);
                assert_eq!(outputs.danmu_outputs.len(), 1);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn pipeline_coordinator_waits_for_danmu_stop_after_video() {
        let coord = PipelineCoordinator::new();
        let session_id = "session1";
        let streamer_id = "streamer1";

        coord.apply_event_inline(configure_event(
            session_id,
            streamer_id,
            true,
            None,
            None,
            Some(empty_session_pipeline()),
        ));
        coord.apply_event_inline(PipelineCoordinationEvent::DanmuCollectionStarted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
        });
        coord.apply_event_inline(PipelineCoordinationEvent::VideoSegmentCompleted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            segment_index: 0,
            path: PathBuf::from("/seg0.flv"),
        });
        coord.apply_event_inline(end_event(session_id, streamer_id));
        assert!(
            coord
                .apply_event_inline(persisted_event(session_id))
                .is_empty()
        );

        let commands =
            coord.apply_event_inline(PipelineCoordinationEvent::DanmuCollectionStopped {
                session_id: session_id.to_string(),
            });

        assert!(matches!(
            commands.as_slice(),
            [PipelineCommand::CreateSessionCompleteDag { .. }]
        ));
    }

    #[test]
    fn pipeline_coordinator_handles_session_end_before_artifacts() {
        let coord = PipelineCoordinator::new();
        let session_id = "session1";
        let streamer_id = "streamer1";

        coord.apply_event_inline(configure_event(
            session_id,
            streamer_id,
            false,
            None,
            None,
            Some(empty_session_pipeline()),
        ));
        coord.apply_event_inline(end_event(session_id, streamer_id));
        assert!(
            coord
                .apply_event_inline(persisted_event(session_id))
                .is_empty()
        );

        let commands = coord.apply_event_inline(PipelineCoordinationEvent::VideoSegmentCompleted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            segment_index: 0,
            path: PathBuf::from("/seg0.flv"),
        });

        assert!(matches!(
            commands.as_slice(),
            [PipelineCommand::CreateSessionCompleteDag { .. }]
        ));
    }

    #[test]
    fn pipeline_coordinator_session_complete_is_exactly_once() {
        let coord = PipelineCoordinator::new();
        let session_id = "session1";
        let streamer_id = "streamer1";

        coord.apply_event_inline(configure_event(
            session_id,
            streamer_id,
            false,
            None,
            None,
            Some(empty_session_pipeline()),
        ));
        coord.apply_event_inline(PipelineCoordinationEvent::VideoSegmentCompleted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            segment_index: 0,
            path: PathBuf::from("/seg0.flv"),
        });
        coord.apply_event_inline(end_event(session_id, streamer_id));
        let commands = coord.apply_event_inline(persisted_event(session_id));
        assert_eq!(commands.len(), 1);

        assert!(
            coord
                .apply_event_inline(persisted_event(session_id))
                .is_empty()
        );
        assert!(
            coord
                .apply_event_inline(PipelineCoordinationEvent::VideoSegmentCompleted {
                    session_id: session_id.to_string(),
                    streamer_id: streamer_id.to_string(),
                    segment_index: 0,
                    path: PathBuf::from("/seg0.flv"),
                })
                .is_empty()
        );
    }

    #[test]
    fn pipeline_coordinator_segment_dag_command_is_idempotent() {
        let coord = PipelineCoordinator::new();
        let session_id = "session1";
        let streamer_id = "streamer1";

        coord.apply_event_inline(configure_event(
            session_id,
            streamer_id,
            false,
            Some(non_empty_pipeline("segment")),
            None,
            Some(empty_session_pipeline()),
        ));
        let commands = coord.apply_event_inline(PipelineCoordinationEvent::VideoSegmentCompleted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            segment_index: 0,
            path: PathBuf::from("/seg0.flv"),
        });
        assert!(matches!(
            commands.as_slice(),
            [PipelineCommand::CreateSegmentDag { .. }]
        ));

        let commands = coord.apply_event_inline(PipelineCoordinationEvent::VideoSegmentCompleted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            segment_index: 0,
            path: PathBuf::from("/seg0.flv"),
        });
        assert!(
            commands.is_empty(),
            "duplicate source artifact must not create another segment DAG"
        );
    }

    #[test]
    fn pipeline_coordinator_distinct_canonical_indices_create_distinct_dags() {
        let coord = PipelineCoordinator::new();
        let session_id = "session1";
        let streamer_id = "streamer1";

        coord.apply_event_inline(configure_event(
            session_id,
            streamer_id,
            false,
            Some(non_empty_pipeline("segment")),
            None,
            Some(empty_session_pipeline()),
        ));

        for (segment_index, path) in [(0, "/seg0.flv"), (1, "/seg1.flv")] {
            let commands =
                coord.apply_event_inline(PipelineCoordinationEvent::VideoSegmentCompleted {
                    session_id: session_id.to_string(),
                    streamer_id: streamer_id.to_string(),
                    segment_index,
                    path: PathBuf::from(path),
                });
            match commands.as_slice() {
                [
                    PipelineCommand::CreateSegmentDag {
                        segment_index: actual_index,
                        input_path,
                        ..
                    },
                ] => {
                    assert_eq!(*actual_index, segment_index);
                    assert_eq!(input_path, &PathBuf::from(path));
                }
                other => panic!("unexpected commands: {other:?}"),
            }
        }
    }

    #[test]
    fn pipeline_coordinator_failed_segment_dag_drains() {
        let coord = PipelineCoordinator::new();
        let session_id = "session1";
        let streamer_id = "streamer1";

        coord.apply_event_inline(configure_event(
            session_id,
            streamer_id,
            false,
            Some(non_empty_pipeline("segment")),
            None,
            Some(empty_session_pipeline()),
        ));
        let commands = coord.apply_event_inline(PipelineCoordinationEvent::VideoSegmentCompleted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            segment_index: 0,
            path: PathBuf::from("/seg0.flv"),
        });
        assert!(matches!(
            commands.as_slice(),
            [PipelineCommand::CreateSegmentDag { .. }]
        ));
        coord.apply_event_inline(PipelineCoordinationEvent::SegmentDagStarted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            segment_index: 0,
            source: SourceType::Video,
        });
        coord.apply_event_inline(PipelineCoordinationEvent::SessionEnded {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            should_run_session_complete: true,
        });
        coord.apply_event_inline(persisted_event(session_id));

        let commands = coord.apply_event_inline(PipelineCoordinationEvent::SegmentDagFailed {
            session_id: session_id.to_string(),
            segment_index: 0,
            source: SourceType::Video,
        });

        assert!(matches!(
            commands.as_slice(),
            [PipelineCommand::CreateSessionCompleteDag { .. }]
        ));
    }

    #[test]
    fn pipeline_coordinator_failed_segment_dag_can_be_retried_before_finalization() {
        let coord = PipelineCoordinator::new();
        let session_id = "session1";
        let streamer_id = "streamer1";

        coord.apply_event_inline(configure_event(
            session_id,
            streamer_id,
            false,
            Some(non_empty_pipeline("segment")),
            None,
            Some(empty_session_pipeline()),
        ));
        let commands = coord.apply_event_inline(PipelineCoordinationEvent::VideoSegmentCompleted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            segment_index: 0,
            path: PathBuf::from("/seg0.flv"),
        });
        assert!(matches!(
            commands.as_slice(),
            [PipelineCommand::CreateSegmentDag { .. }]
        ));
        coord.apply_event_inline(PipelineCoordinationEvent::SegmentDagStarted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            segment_index: 0,
            source: SourceType::Video,
        });

        assert!(
            coord
                .apply_event_inline(PipelineCoordinationEvent::SegmentDagFailed {
                    session_id: session_id.to_string(),
                    segment_index: 0,
                    source: SourceType::Video,
                })
                .is_empty()
        );

        coord.apply_event_inline(PipelineCoordinationEvent::SegmentDagStarted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            segment_index: 0,
            source: SourceType::Video,
        });
        coord.apply_event_inline(PipelineCoordinationEvent::SessionEnded {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            should_run_session_complete: true,
        });
        assert!(
            coord
                .apply_event_inline(persisted_event(session_id))
                .is_empty(),
            "retried segment DAG is in flight, so fallback output must not finalize the session"
        );

        let commands = coord.apply_event_inline(PipelineCoordinationEvent::SegmentDagCompleted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            segment_index: 0,
            source: SourceType::Video,
            outputs: vec![PathBuf::from("/seg0.mp4")],
        });

        match commands.as_slice() {
            [PipelineCommand::CreateSessionCompleteDag { outputs, .. }] => {
                assert_eq!(
                    outputs.get_sorted_video_outputs(),
                    [PathBuf::from("/seg0.mp4")]
                );
            }
            other => panic!("unexpected commands: {other:?}"),
        }
    }

    #[test]
    fn pipeline_coordinator_paired_pipeline_waits_for_same_index() {
        let coord = PipelineCoordinator::new();
        let session_id = "session1";
        let streamer_id = "streamer1";

        coord.apply_event_inline(configure_event(
            session_id,
            streamer_id,
            true,
            None,
            Some(non_empty_pipeline("paired")),
            None,
        ));
        assert!(
            coord
                .apply_event_inline(PipelineCoordinationEvent::VideoSegmentCompleted {
                    session_id: session_id.to_string(),
                    streamer_id: streamer_id.to_string(),
                    segment_index: 0,
                    path: PathBuf::from("/seg0.flv"),
                })
                .is_empty()
        );
        assert!(
            coord
                .apply_event_inline(PipelineCoordinationEvent::DanmuSegmentCompleted {
                    session_id: session_id.to_string(),
                    streamer_id: streamer_id.to_string(),
                    segment_index: 1,
                    path: PathBuf::from("/seg1.xml"),
                })
                .is_empty()
        );

        let commands = coord.apply_event_inline(PipelineCoordinationEvent::DanmuSegmentCompleted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            segment_index: 0,
            path: PathBuf::from("/seg0.xml"),
        });
        assert!(matches!(
            commands.as_slice(),
            [PipelineCommand::CreatePairedSegmentDag { .. }]
        ));
    }

    /// When the last segment's video DAG completes after the session has
    /// already ended and persisted, `try_trigger_paired` emits a paired
    /// command and `try_finalize` runs in the same reducer call. The
    /// session-complete DAG must not fire alongside the paired DAG.
    /// `pending_paired_starts` is the gate: it holds the segment index
    /// from the moment `try_trigger_paired` emits until
    /// `on_paired_dag_started` removes it.
    #[test]
    fn pipeline_coordinator_session_complete_waits_for_just_triggered_paired() {
        let coord = PipelineCoordinator::new();
        let session_id = "session1";
        let streamer_id = "streamer1";

        coord.apply_event_inline(configure_event(
            session_id,
            streamer_id,
            true,
            Some(non_empty_pipeline("segment")),
            Some(non_empty_pipeline("paired")),
            Some(empty_session_pipeline()),
        ));
        coord.apply_event_inline(PipelineCoordinationEvent::DanmuCollectionStarted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
        });

        // Source artifacts kick off per-segment DAGs.
        let commands = coord.apply_event_inline(PipelineCoordinationEvent::VideoSegmentCompleted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            segment_index: 0,
            path: PathBuf::from("/seg0.flv"),
        });
        assert!(matches!(
            commands.as_slice(),
            [PipelineCommand::CreateSegmentDag { .. }]
        ));
        let commands = coord.apply_event_inline(PipelineCoordinationEvent::DanmuSegmentCompleted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            segment_index: 0,
            path: PathBuf::from("/seg0.xml"),
        });
        assert!(matches!(
            commands.as_slice(),
            [PipelineCommand::CreateSegmentDag { .. }]
        ));

        // Both per-segment DAGs are in flight.
        coord.apply_event_inline(PipelineCoordinationEvent::SegmentDagStarted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            segment_index: 0,
            source: SourceType::Video,
        });
        coord.apply_event_inline(PipelineCoordinationEvent::SegmentDagStarted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            segment_index: 0,
            source: SourceType::Danmu,
        });

        // Session wraps up while the per-segment DAGs are still running.
        assert!(
            coord
                .apply_event_inline(PipelineCoordinationEvent::DanmuCollectionStopped {
                    session_id: session_id.to_string(),
                })
                .is_empty()
        );
        assert!(
            coord
                .apply_event_inline(end_event(session_id, streamer_id))
                .is_empty()
        );
        assert!(
            coord
                .apply_event_inline(persisted_event(session_id))
                .is_empty()
        );

        // Danmu DAG drains first. Video is still in flight, so neither
        // a paired command nor session-complete can fire.
        let commands = coord.apply_event_inline(PipelineCoordinationEvent::SegmentDagCompleted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            segment_index: 0,
            source: SourceType::Danmu,
            outputs: vec![PathBuf::from("/seg0.xml")],
        });
        assert!(
            commands.is_empty(),
            "danmu drain alone must not trigger paired or session-complete: {commands:?}"
        );

        // Video DAG drains. `try_trigger_paired` emits the paired
        // command; `try_finalize` runs in the same call but must NOT
        // emit session-complete because `pending_paired_starts` now
        // holds segment 0.
        let commands = coord.apply_event_inline(PipelineCoordinationEvent::SegmentDagCompleted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            segment_index: 0,
            source: SourceType::Video,
            outputs: vec![PathBuf::from("/seg0.mp4")],
        });
        match commands.as_slice() {
            [PipelineCommand::CreatePairedSegmentDag { outputs, .. }] => {
                assert_eq!(outputs.segment_index, 0);
            }
            other => panic!(
                "expected only the paired command; session-complete must wait: {other:?}"
            ),
        }

        // `PairedDagStarted` clears `pending_paired_starts`, but
        // `pending_paired_dags` becomes 1, so session-complete still
        // waits.
        coord.apply_event_inline(PipelineCoordinationEvent::PairedDagStarted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            segment_index: 0,
        });

        // Paired DAG finishes -> all gates clear -> session-complete fires.
        let commands = coord.apply_event_inline(PipelineCoordinationEvent::PairedDagCompleted {
            session_id: session_id.to_string(),
        });
        assert!(matches!(
            commands.as_slice(),
            [PipelineCommand::CreateSessionCompleteDag { .. }]
        ));
    }
}
