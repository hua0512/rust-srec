//! Awaiting quiescence of a session's pipeline work.

use std::time::Duration;

use super::*;
use crate::pipeline::coordination::SessionCoordinationOutstanding;

/// First gap between two [`PipelineManager::drain_for_session`] observations.
const DRAIN_POLL_MIN: Duration = Duration::from_millis(100);

/// Ceiling the gap backs off to. Post-processing steps run for seconds to
/// minutes, so a longer drain settles for one observation every two seconds
/// rather than paying the query cost of the fast first checks throughout.
const DRAIN_POLL_MAX: Duration = Duration::from_secs(2);

/// Recording sessions whose dispatch receipt or pipeline work is unfinished.
#[derive(Debug, Default)]
pub struct StreamerDrain {
    pub sessions: Vec<String>,
    pub jobs: u64,
}

impl StreamerDrain {
    pub fn is_drained(&self) -> bool {
        self.sessions.is_empty() && self.jobs == 0
    }
}

/// Pipeline work still attached to a session.
///
/// The three sources are deliberately combined because each is blind to a window
/// the others cover; see [`PipelineManager::outstanding_for_session`] for the
/// order they have to be read in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionPipelineOutstanding {
    /// `job` rows carrying this `session_id` in `PENDING` or `PROCESSING`.
    pub jobs: u64,
    /// `dag_execution` rows carrying this `session_id` in `PENDING` or
    /// `PROCESSING`.
    pub dags: u64,
    /// What `PipelineCoordinator` still holds for the session; all zero when it
    /// tracks no such session.
    pub coordinated: SessionCoordinationOutstanding,
}

impl SessionPipelineOutstanding {
    /// Whether no pipeline work is attached to the session any more.
    pub fn is_quiescent(&self) -> bool {
        self.jobs == 0 && self.dags == 0 && self.coordinated.is_idle()
    }
}

/// How a [`PipelineManager::drain_for_session`] call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionDrain {
    /// The session had no pipeline work left, or all of it finished inside the
    /// caller's bound.
    Drained,
    /// The bound expired. Carries the last observation so the caller can name
    /// what it gave up on.
    Outstanding(SessionPipelineOutstanding),
}

impl SessionDrain {
    /// Whether the session reached quiescence.
    pub fn is_drained(&self) -> bool {
        matches!(self, Self::Drained)
    }
}

impl<CR, SR> PipelineManager<CR, SR>
where
    CR: ConfigRepository + Send + Sync + 'static,
    SR: StreamerRepository + Send + Sync + 'static,
{
    /// Observe every session that can still need this streamer's configuration.
    /// Runtime callers stop producers first. Retry an already-observed final
    /// dispatch when its dependencies have settled. Only a durable dispatch/skip
    /// receipt acknowledges the end consumer; cache retention cannot do that.
    pub async fn drain_for_streamer(
        &self,
        streamer_id: &str,
        bound: Duration,
    ) -> Result<StreamerDrain> {
        let repo = self.session_repo.as_ref().ok_or_else(|| {
            Error::Validation(
                "Session repository is required to acknowledge streamer retirement".to_string(),
            )
        })?;
        let deadline = tokio::time::Instant::now().checked_add(bound);
        let mut result = StreamerDrain::default();
        for session in repo.list_sessions_for_retirement(streamer_id).await? {
            if let Some(outstanding) = self
                .pipeline_coordinator
                .session_outstanding(&session.id)
                .await
                && outstanding.end_processed
                && outstanding.session_complete_owed
                && outstanding.pending_dags == 0
                && !repo.session_pipeline_settled(&session.id).await?
            {
                let commands = self
                    .pipeline_coordinator
                    .apply_event(PipelineCoordinationEvent::SessionEndPersisted {
                        session_id: session.id.clone(),
                    })
                    .await;
                self.execute_pipeline_commands(commands).await;
                self.record_session_pipeline_settlement(&session.id).await;
            }
            let settled = repo.session_pipeline_settled(&session.id).await?;
            let remaining = deadline
                .map(|deadline| deadline.saturating_duration_since(tokio::time::Instant::now()))
                .unwrap_or(bound);
            let drained = self.drain_for_session(&session.id, remaining).await?;
            if !settled || !drained.is_drained() {
                result.sessions.push(session.id);
            }
        }
        // Standalone jobs, or jobs whose session was explicitly removed, still
        // carry the streamer id and must finish before its metadata is removed.
        if let Some(repo) = &self.job_repository {
            result.jobs = repo
                .count_jobs(
                    &JobFilters::new()
                        .with_streamer_id(streamer_id)
                        .with_statuses([JobStatus::Pending, JobStatus::Processing]),
                )
                .await?;
        }
        Ok(result)
    }

    /// Wait until no pipeline work is attached to `session_id`, giving up after
    /// `bound`.
    ///
    /// This waits for in-flight work rather than cancelling it: a step that is
    /// most of the way through a transcode or an upload still produces the file
    /// the user asked for, and throwing it away is a policy decision that belongs
    /// to the caller, which can reach for [`Self::cancel_dag`] or
    /// [`Self::cancel_pipeline`] when a drain reports outstanding work.
    ///
    /// `bound` is the caller's; `Duration::ZERO` makes this a single observation
    /// and a `bound` too large to add to the clock makes it unbounded.
    ///
    /// Work that has not been recorded anywhere yet is not covered: a segment the
    /// downloader completes after quiescence is observed starts a new DAG, so
    /// callers must first stop whatever would create more work for the session.
    /// Once `PipelineCoordinator` has recorded that a session ended,
    /// [`SessionCoordinationOutstanding::session_complete_owed`] does cover the
    /// session-complete DAG that has not been created yet.
    ///
    /// Neither `job` nor `dag_execution` has a foreign key to `live_sessions` or
    /// `streamers`, so their rows survive the deletion of either and this call
    /// keeps reporting on them afterwards. Nothing here reads `live_sessions`, so
    /// a session id that no longer has a row - or never had one - drains as
    /// normal.
    pub async fn drain_for_session(
        &self,
        session_id: &str,
        bound: Duration,
    ) -> Result<SessionDrain> {
        // `None` when `bound` overflows the clock, which reads as "no deadline"
        // below rather than panicking on the addition.
        let deadline = tokio::time::Instant::now().checked_add(bound);
        let mut gap = DRAIN_POLL_MIN;

        loop {
            let outstanding = self.outstanding_for_session(session_id).await?;
            if outstanding.is_quiescent() {
                return Ok(SessionDrain::Drained);
            }

            // Checked after the observation so a zero bound still reports one.
            let now = tokio::time::Instant::now();
            let remaining = match deadline {
                Some(deadline) if now >= deadline => {
                    warn!(
                        session_id = %session_id,
                        jobs = %outstanding.jobs,
                        dags = %outstanding.dags,
                        coordinated_dags = %outstanding.coordinated.pending_dags,
                        session_complete_owed = %outstanding.coordinated.session_complete_owed,
                        "Session pipeline drain bound expired with work still in flight"
                    );
                    return Ok(SessionDrain::Outstanding(outstanding));
                }
                Some(deadline) => Some(deadline - now),
                None => None,
            };

            // Clamped to what is left of the bound, so the next observation lands
            // no later than the deadline.
            let sleep = match remaining {
                Some(remaining) => gap.min(remaining),
                None => gap,
            };
            tokio::time::sleep(sleep).await;
            gap = (gap * 2).min(DRAIN_POLL_MAX);
        }
    }

    /// One observation of the pipeline work attached to `session_id`.
    ///
    /// Read order is load-bearing, because the three sources are updated in a
    /// fixed order and a later read must never be able to miss what an earlier
    /// one already ruled out:
    ///
    /// - Creation is recorded in the coordinator before any row exists, for every
    ///   kind of DAG: `try_finalize` sets `pending_session_complete_start` before
    ///   emitting `CreateSessionCompleteDag`, `try_trigger_paired` fills
    ///   `pending_paired_starts` before emitting `CreatePairedSegmentDag`, and
    ///   `on_source_artifact` fills `pending_segment_starts` before emitting
    ///   `CreateSegmentDag` - all three are part of what
    ///   `PipelineCoordinator::session_outstanding` reports. Reading the
    ///   coordinator first therefore means that whenever it reports nothing
    ///   outstanding, no DAG has been commanded whose rows do not exist yet, and
    ///   the later reads see every row that does.
    /// - Completion runs row-first: a `job` reaches a terminal status before its
    ///   `dag_execution` row does, and the row before `handle_dag_completion`
    ///   reaches the coordinator. Reading jobs last therefore means a zero count
    ///   is consistent with the DAG and coordinator reads that preceded it.
    ///
    /// The `job` and `dag_execution` counts are zero when the manager was built
    /// without the corresponding repository, which is how the non-persistent
    /// configurations report "nothing to wait for".
    pub async fn outstanding_for_session(
        &self,
        session_id: &str,
    ) -> Result<SessionPipelineOutstanding> {
        let coordinated = self
            .pipeline_coordinator
            .session_outstanding(session_id)
            .await
            .unwrap_or_default();

        let dags = match &self.dag_scheduler {
            Some(scheduler) => {
                // `DagRepository::count_dags` takes one status, and the DAG
                // statuses that are not terminal are exactly these two.
                let mut dags = 0;
                for status in [DagExecutionStatus::Pending, DagExecutionStatus::Processing] {
                    dags += scheduler
                        .count_dags(Some(status.as_str()), Some(session_id))
                        .await?;
                }
                dags
            }
            None => 0,
        };

        let jobs = match &self.job_repository {
            Some(repo) => {
                repo.count_jobs(
                    &JobFilters::new()
                        .with_session_id(session_id)
                        .with_statuses([JobStatus::Pending, JobStatus::Processing]),
                )
                .await?
            }
            None => 0,
        };

        Ok(SessionPipelineOutstanding {
            jobs,
            dags,
            coordinated,
        })
    }
}
