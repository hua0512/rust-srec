//! Per-session reducer state (`SessionPipelineState`, `SegmentState`, `ArtifactLane`)
//! for `PipelineCoordinator`.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use tracing::{debug, info, trace, warn};

use super::{
    PairedSegmentOutputs, PipelineCommand, SegmentOutput, SessionCoordinationOutstanding,
    SessionOutputs, SourceType, dedup_paths_preserve_order,
};
use crate::database::models::job::DagPipelineDefinition;

#[derive(Debug)]
pub struct SessionPipelineState {
    pub(super) session_id: String,
    pub(super) streamer_id: String,
    pub(super) configured: bool,
    pub(super) danmu_expected: bool,
    pub(super) danmu_observed: bool,
    pub(super) video_complete: bool,
    pub(super) danmu_complete: bool,
    pub(super) session_end_observed: bool,
    pub(super) session_end_persisted: bool,
    pub(super) session_complete_triggered: bool,
    /// Set when the final DAG was published or `try_finalize` found no
    /// definition. Publication failures clear this flag so retirement retains
    /// the dispatch and its configuration for another attempt.
    pub(super) session_complete_settled: bool,
    pub(super) pending_session_complete_start: bool,
    /// First monotonic observation by the reducer. Finalization copies this
    /// into the dispatch snapshot alongside its last-activity time.
    pub(super) created_at: Instant,
    pub(super) last_activity: Instant,
    pub(super) segment_pipeline: Option<DagPipelineDefinition>,
    pub(super) paired_segment_pipeline: Option<DagPipelineDefinition>,
    pub(super) session_complete_pipeline: Option<DagPipelineDefinition>,
    pub(super) segments: BTreeMap<u32, SegmentState>,
    pub(super) pending_video_dags: u32,
    pub(super) pending_danmu_dags: u32,
    pub(super) pending_paired_dags: u32,
    pub(super) started_segment_dags: HashSet<(SourceType, u32)>,
    pub(super) started_paired_dags: HashSet<u32>,
    pub(super) triggered_paired_segments: HashSet<u32>,
    /// Segment indices for which `try_trigger_paired` emitted a
    /// `CreatePairedSegmentDag` command but `on_paired_dag_started` has not
    /// yet been applied. Without this set, `try_finalize` can fire
    /// session-complete in the same reducer call that just emitted the
    /// paired command, because `pending_paired_dags` only counts started
    /// DAGs. The gate in `artifacts_drained_for_session_complete` rejects
    /// finalization while this set is non-empty.
    pub(super) pending_paired_starts: HashSet<u32>,
    /// Segments for which `on_source_artifact` / `recover_source_artifact` emitted
    /// a `CreateSegmentDag` command but `on_segment_dag_started` has not been
    /// applied. `PipelineManager::run_segment_pipeline` applies
    /// `SegmentDagStarted` before `create_dag_pipeline_internal` publishes any
    /// row, so this is the only record of a segment DAG in that window.
    ///
    /// Unlike `pending_paired_starts` this does not gate `try_finalize`; it
    /// exists so `SessionCoordinationOutstanding::pending_dags` stays a
    /// time-superset of the `dag_execution` rows, which is what lets
    /// `PipelineManager::outstanding_for_session` read the coordinator first.
    ///
    /// An entry is only ever cleared by the command it was inserted for reaching
    /// `PipelineManager::execute_pipeline_commands`, so every emitter has to hand
    /// its commands on: `handle_recorder_notification` and `recover_sessions` both do.
    /// A command emitted and then dropped leaves the session reported as busy for
    /// the length of a caller's `drain_for_session` bound.
    pub(super) pending_segment_starts: HashSet<(SourceType, u32)>,
}

impl SessionPipelineState {
    pub(super) fn new(session_id: String, streamer_id: String) -> Self {
        let now = Instant::now();
        Self {
            session_id,
            streamer_id,
            configured: false,
            danmu_expected: false,
            danmu_observed: false,
            video_complete: false,
            danmu_complete: false,
            session_end_observed: false,
            session_end_persisted: false,
            session_complete_triggered: false,
            session_complete_settled: false,
            pending_session_complete_start: false,
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
            pending_segment_starts: HashSet::new(),
        }
    }

    pub(super) fn configure(
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
        self.configured = true;
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

    pub(super) fn segment_mut(&mut self, segment_index: u32) -> &mut SegmentState {
        self.segments.entry(segment_index).or_default()
    }

    pub(super) fn on_source_artifact(
        &mut self,
        source: SourceType,
        segment_index: u32,
        path: PathBuf,
    ) -> Vec<PipelineCommand> {
        self.last_activity = Instant::now();
        let has_segment_pipeline = self
            .segment_pipeline
            .as_ref()
            .is_some_and(|pipeline| !pipeline.is_empty());

        if source == SourceType::Danmu {
            self.danmu_observed = true;
        }

        if has_segment_pipeline {
            return self.start_segment_dag(source, segment_index, path);
        }
        self.segment_mut(segment_index)
            .artifact_mut(source)
            .add_final(path);
        let mut commands = self.try_trigger_paired(segment_index);
        commands.extend(self.try_finalize());
        commands
    }

    pub(super) fn recover_source_artifact(
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

        if has_segment_pipeline {
            return self.start_segment_dag(source, segment_index, path);
        }

        let artifact = self.segment_mut(segment_index).artifact_mut(source);
        artifact.add_source(path.clone());
        artifact.add_final(path);
        let mut commands = self.try_trigger_paired(segment_index);
        commands.extend(self.try_finalize());
        commands
    }

    /// Record an artifact for a configured segment pipeline and reserve its
    /// publication exactly once. Replayed sources after failure remain fallback
    /// inputs, without opening another DAG or clearing the pending-start gate.
    fn start_segment_dag(
        &mut self,
        source: SourceType,
        segment_index: u32,
        path: PathBuf,
    ) -> Vec<PipelineCommand> {
        let should_start = {
            let artifact = self.segment_mut(segment_index).artifact_mut(source);
            artifact.add_source(path.clone());
            if artifact.dag_started {
                if artifact.dag_failed {
                    artifact.use_source_inputs_as_failed_fallback();
                }
                false
            } else {
                artifact.dag_started = true;
                true
            }
        };
        if should_start && let Some(pipeline) = self.segment_pipeline.clone() {
            self.pending_segment_starts.insert((source, segment_index));
            return vec![PipelineCommand::CreateSegmentDag {
                session_id: self.session_id.clone(),
                streamer_id: self.streamer_id.clone(),
                segment_index,
                source,
                input_path: path,
                pipeline,
            }];
        }
        Vec::new()
    }

    pub(super) fn recover_segment_dag_completed(
        &mut self,
        source: SourceType,
        segment_index: u32,
        outputs: Vec<PathBuf>,
    ) -> Vec<PipelineCommand> {
        self.last_activity = Instant::now();
        if source == SourceType::Danmu {
            self.danmu_observed = true;
        }

        // The DAG's outcome is known, so nothing is waiting on a start event for
        // it any more.
        self.pending_segment_starts.remove(&(source, segment_index));

        let outputs = dedup_paths_preserve_order(outputs);
        let artifact = self.segment_mut(segment_index).artifact_mut(source);
        artifact.dag_started = true;
        artifact.dag_failed = false;
        for output in outputs {
            artifact.add_final(output);
        }

        let mut commands = self.try_trigger_paired(segment_index);
        commands.extend(self.try_finalize());
        commands
    }

    pub(super) fn recover_segment_dag_failed(
        &mut self,
        source: SourceType,
        segment_index: u32,
    ) -> Vec<PipelineCommand> {
        self.last_activity = Instant::now();
        if source == SourceType::Danmu {
            self.danmu_observed = true;
        }

        self.pending_segment_starts.remove(&(source, segment_index));

        let artifact = self.segment_mut(segment_index).artifact_mut(source);
        artifact.dag_started = true;
        artifact.dag_failed = true;
        artifact.use_source_inputs_as_failed_fallback();

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

    pub(super) fn on_segment_dag_started(&mut self, source: SourceType, segment_index: u32) {
        self.last_activity = Instant::now();
        // Removing an absent key is a no-op, so a recovery-driven
        // `SegmentDagStarted` with no `CreateSegmentDag` behind it in this
        // process does not desync the set.
        self.pending_segment_starts.remove(&(source, segment_index));
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

    pub(super) fn on_segment_dag_completed(
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
        artifact.dag_failed = false;
        for output in outputs {
            artifact.add_final(output);
        }

        let mut commands = self.try_trigger_paired(segment_index);
        commands.extend(self.try_finalize());
        commands
    }

    pub(super) fn on_segment_dag_failed(
        &mut self,
        source: SourceType,
        segment_index: u32,
    ) -> Vec<PipelineCommand> {
        self.last_activity = Instant::now();
        self.decrement_segment_pending(source, segment_index, "failed");

        if let Some(segment) = self.segments.get_mut(&segment_index) {
            let artifact = segment.artifact_mut(source);
            artifact.dag_failed = true;
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
        if source == SourceType::Danmu {
            self.danmu_observed = true;
        }
        // The counter was raised by the start that inserted this key. A completion
        // without one, such as a retried DAG whose failed run already settled the
        // segment, changes the artifacts but owes no decrement.
        if !self.started_segment_dags.remove(&(source, segment_index)) {
            debug!(
                session_id = %self.session_id,
                segment_index = %segment_index,
                source = ?source,
                reason = %reason,
                "Segment DAG finished without a pending start"
            );
            return;
        }
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

    pub(super) fn on_paired_dag_started(&mut self, segment_index: u32) {
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

    pub(super) fn recover_paired_dag_triggered(&mut self, segment_index: u32) {
        self.last_activity = Instant::now();
        self.triggered_paired_segments.insert(segment_index);
    }

    pub(super) fn on_paired_dag_finished(&mut self, succeeded: bool) -> Vec<PipelineCommand> {
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

    pub(super) fn try_finalize(&mut self) -> Vec<PipelineCommand> {
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
            self.session_complete_settled = true;
            return Vec::new();
        };

        self.pending_session_complete_start = true;
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
            && !self.pending_session_complete_start
            && self.artifacts_drained_for_session_complete()
    }

    /// The finalization gate conditions still unmet, for diagnosing a session
    /// that never reached `is_ready`.
    ///
    /// Mirrors `is_ready` and `artifacts_drained_for_session_complete`; keep the
    /// two in step when either gains a condition.
    pub(super) fn unmet_completion_conditions(&self) -> Vec<&'static str> {
        let mut unmet = Vec::new();
        if !self.session_end_observed {
            unmet.push("session_end_observed");
        }
        if !self.session_end_persisted {
            unmet.push("session_end_persisted");
        }
        if self.session_complete_triggered {
            unmet.push("session_complete_triggered");
        }
        if self.pending_session_complete_start {
            unmet.push("pending_session_complete_start");
        }
        if !self.video_complete {
            unmet.push("video_complete");
        }
        if self.pending_video_dags != 0 {
            unmet.push("pending_video_dags");
        }
        if self.danmu_expected && self.danmu_observed {
            if !self.danmu_complete {
                unmet.push("danmu_complete");
            }
            if self.pending_danmu_dags != 0 {
                unmet.push("pending_danmu_dags");
            }
        }
        if self.pending_paired_dags != 0 {
            unmet.push("pending_paired_dags");
        }
        if !self.pending_paired_starts.is_empty() {
            unmet.push("pending_paired_starts");
        }
        if !self.has_video_output() {
            unmet.push("has_video_output");
        }
        unmet
    }

    /// The DAG work this session is still waiting on, in the same terms
    /// `artifacts_drained_for_session_complete` and `is_ready` gate
    /// finalization on.
    pub(super) fn outstanding(&self) -> SessionCoordinationOutstanding {
        let pending_dags = self.pending_dag_count();
        SessionCoordinationOutstanding {
            end_processed: self.session_end_persisted,
            pending_dags,
            // `session_end_observed` is set only when `SessionEnded` carried
            // `should_run_session_complete`.
            session_complete_owed: self.session_end_observed
                && !self.session_complete_settled
                && self.session_complete_possible(pending_dags),
        }
    }

    /// Segment, danmu and paired DAGs this session has started or commanded and
    /// not yet seen finish.
    fn pending_dag_count(&self) -> u32 {
        self.pending_video_dags
            + self.pending_danmu_dags
            + self.pending_paired_dags
            + self.pending_paired_starts.len() as u32
            + self.pending_segment_starts.len() as u32
    }

    /// Whether `try_finalize` can still emit a `CreateSessionCompleteDag`.
    ///
    /// `is_ready` runs through `artifacts_drained_for_session_complete`, which
    /// requires `has_video_output`; only `on_segment_dag_completed` and the
    /// recovery paths can make that true, and they need a DAG to report. So once
    /// the session has ended with no video output recorded and no DAG in flight
    /// or commanded, the gate can never open and nothing further is owed. This is
    /// reached by any session that ended before a segment DAG completed -
    /// `TerminalCause::should_run_session_complete_pipeline` is true for `Failed`,
    /// `StreamerOffline`, `UserDisabled` and `OutOfSchedule` alike.
    ///
    /// A gate left unsatisfiable for one of the other reasons
    /// `unmet_completion_conditions` reports - danmu that is expected and
    /// observed but never completes, say - still reads as owed. Such a session
    /// keeps at least one `pending_*` count or an unmet danmu flag that a caller
    /// can see in the drain report.
    fn session_complete_possible(&self, pending_dags: u32) -> bool {
        self.pending_session_complete_start || self.has_video_output() || pending_dags > 0
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

    #[cfg(test)]
    pub(super) fn active_pair_count(&self) -> usize {
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
            pending_session_complete_start = %self.pending_session_complete_start,
            "Session not ready for session-complete trigger"
        );
    }
}

#[derive(Debug, Default)]
pub struct SegmentState {
    pub(super) video: ArtifactLane,
    pub(super) danmu: ArtifactLane,
}

impl SegmentState {
    pub(super) fn artifact_mut(&mut self, source: SourceType) -> &mut ArtifactLane {
        match source {
            SourceType::Video => &mut self.video,
            SourceType::Danmu => &mut self.danmu,
        }
    }
}

#[derive(Debug, Default)]
pub struct ArtifactLane {
    pub(super) source_inputs: Vec<PathBuf>,
    pub(super) final_outputs: Vec<PathBuf>,
    pub(super) dag_started: bool,
    pub(super) dag_failed: bool,
    pub(super) final_outputs_are_fallback: bool,
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
