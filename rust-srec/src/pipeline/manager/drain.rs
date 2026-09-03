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

/// Pipeline work still attached to a session.
///
/// The three sources are deliberately combined: `dag_execution` rows go terminal
/// before `PipelineManager::handle_dag_completion` tells the coordinator, and the
/// coordinator emits a `CreateSessionCompleteDag` command before any row for it
/// exists, so each source is blind to a window the others cover.
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
    /// Wait until no pipeline work is attached to `session_id`, giving up after
    /// `bound`.
    ///
    /// This waits for in-flight work rather than cancelling it: a step that is
    /// most of the way through a transcode or an upload still produces the file
    /// the user asked for, and throwing it away is a policy decision that belongs
    /// to the caller, which can reach for [`Self::cancel_dag`] or
    /// [`Self::cancel_pipeline`] when a drain reports outstanding work.
    ///
    /// `bound` is the caller's; `Duration::ZERO` makes this a single observation.
    /// Work that starts after quiescence is observed is not covered - callers that
    /// need the session to stay quiescent must first stop whatever would create
    /// more work for it.
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
        let deadline = tokio::time::Instant::now() + bound;
        let mut gap = DRAIN_POLL_MIN;

        loop {
            let outstanding = self.outstanding_for_session(session_id).await?;
            if outstanding.is_quiescent() {
                return Ok(SessionDrain::Drained);
            }

            // Checked after the observation so a zero bound still reports one.
            let now = tokio::time::Instant::now();
            if now >= deadline {
                debug!(
                    session_id = %session_id,
                    jobs = %outstanding.jobs,
                    dags = %outstanding.dags,
                    coordinated_dags = %outstanding.coordinated.pending_dags,
                    session_complete_pending = %outstanding.coordinated.session_complete_pending,
                    "Session pipeline drain bound expired"
                );
                return Ok(SessionDrain::Outstanding(outstanding));
            }

            // Never longer than what is left of the bound, so the loop cannot
            // overshoot the deadline by up to `DRAIN_POLL_MAX`.
            tokio::time::sleep_until(now + gap.min(deadline - now)).await;
            gap = (gap * 2).min(DRAIN_POLL_MAX);
        }
    }

    /// One observation of the pipeline work attached to `session_id`.
    ///
    /// The `job` and `dag_execution` counts are zero when the manager was built
    /// without the corresponding repository, which is how the non-persistent
    /// configurations report "nothing to wait for".
    pub async fn outstanding_for_session(
        &self,
        session_id: &str,
    ) -> Result<SessionPipelineOutstanding> {
        let jobs = match &self.job_repository {
            Some(repo) => {
                let mut jobs = 0;
                // `JobFilters::status` holds one status, and the job statuses
                // that are not terminal are exactly these two.
                for status in [JobStatus::Pending, JobStatus::Processing] {
                    jobs += repo
                        .count_jobs(
                            &JobFilters::new()
                                .with_session_id(session_id)
                                .with_status(status),
                        )
                        .await?;
                }
                jobs
            }
            None => 0,
        };

        let dags = match &self.dag_scheduler {
            Some(scheduler) => {
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

        Ok(SessionPipelineOutstanding {
            jobs,
            dags,
            coordinated: self
                .pipeline_coordinator
                .session_outstanding(session_id)
                .await
                .unwrap_or_default(),
        })
    }
}
