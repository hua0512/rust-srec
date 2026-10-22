use super::*;
use crate::database::models::{
    DagExecutionDbModel, DagExecutionListSummary, DagStepExecutionDbModel, DagStepWithJob,
    DanmuStatisticsDbModel, JobDbModel, JobExecutionLogDbModel, LiveSessionDbModel,
    MediaOutputTypeSummary, OutputFilters, ReadyStep, SessionFilters, SessionSegmentDbModel,
};
use async_trait::async_trait;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

mod danmu_recovery;

mod locality;
mod publication;
mod recovery_status;

struct TestSessionRepository {
    end_time: Mutex<Option<i64>>,
    sessions: Mutex<HashMap<String, LiveSessionDbModel>>,
    segments: Mutex<HashMap<String, Vec<SessionSegmentDbModel>>>,
    outputs: Mutex<HashMap<String, Vec<MediaOutputDbModel>>>,
    list_filters: Mutex<Vec<SessionFilters>>,
    session_complete_dispatched: Mutex<HashSet<String>>,
}

impl TestSessionRepository {
    fn new(end_time: Option<i64>) -> Self {
        Self {
            end_time: Mutex::new(end_time),
            sessions: Mutex::new(HashMap::new()),
            segments: Mutex::new(HashMap::new()),
            outputs: Mutex::new(HashMap::new()),
            list_filters: Mutex::new(Vec::new()),
            session_complete_dispatched: Mutex::new(HashSet::new()),
        }
    }

    fn insert_session(&self, session: LiveSessionDbModel) {
        self.sessions
            .lock()
            .expect("lock poisoned")
            .insert(session.id.clone(), session);
    }

    fn insert_segment(&self, segment: SessionSegmentDbModel) {
        self.segments
            .lock()
            .expect("lock poisoned")
            .entry(segment.session_id.clone())
            .or_default()
            .push(segment);
    }

    fn insert_output(&self, output: MediaOutputDbModel) {
        self.outputs
            .lock()
            .expect("lock poisoned")
            .entry(output.session_id.clone())
            .or_default()
            .push(output);
    }

    fn list_filters(&self) -> Vec<SessionFilters> {
        self.list_filters.lock().expect("lock poisoned").clone()
    }
}

#[async_trait]
impl SessionRepository for TestSessionRepository {
    async fn get_session(&self, id: &str) -> Result<LiveSessionDbModel> {
        if let Some(session) = self
            .sessions
            .lock()
            .expect("lock poisoned")
            .get(id)
            .cloned()
        {
            return Ok(session);
        }

        Ok(LiveSessionDbModel {
            id: id.to_string(),
            streamer_id: Some("streamer-1".to_string()),
            streamer_name: Some("Streamer One".to_string()),
            start_time: chrono::Utc::now().timestamp_millis(),
            end_time: *self.end_time.lock().expect("lock poisoned"),
            titles: Some("[]".to_string()),
            total_size_bytes: 0,
        })
    }

    async fn get_active_session_for_streamer(
        &self,
        _streamer_id: &str,
    ) -> Result<Option<LiveSessionDbModel>> {
        unimplemented!("not needed for these tests")
    }

    async fn list_sessions_for_streamer(
        &self,
        _streamer_id: &str,
        _limit: i32,
    ) -> Result<Vec<LiveSessionDbModel>> {
        unimplemented!("not needed for these tests")
    }

    async fn create_session(&self, _session: &LiveSessionDbModel) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn end_session(&self, _id: &str, _end_time: i64) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn update_session_titles(&self, _id: &str, _titles: &str) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn delete_session(&self, _id: &str) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn delete_sessions_batch(&self, _ids: &[String]) -> Result<u64> {
        unimplemented!("not needed for these tests")
    }

    async fn list_sessions_filtered(
        &self,
        filters: &SessionFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<LiveSessionDbModel>, u64)> {
        self.list_filters
            .lock()
            .expect("lock poisoned")
            .push(filters.clone());

        let mut sessions = self
            .sessions
            .lock()
            .expect("lock poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();

        if let Some(streamer_id) = &filters.streamer_id {
            sessions.retain(|session| session.streamer_id.as_ref() == Some(streamer_id));
        }
        if filters.active_only == Some(true) {
            sessions.retain(|session| session.end_time.is_none());
        }
        if filters.include_empty != Some(true) {
            sessions.retain(|session| session.total_size_bytes > 0 || session.end_time.is_none());
        }
        sessions.sort_by_key(|session| Reverse(session.start_time));

        let total = sessions.len() as u64;
        let start = pagination.offset as usize;
        let end = start.saturating_add(pagination.limit as usize);
        Ok((
            sessions
                .into_iter()
                .skip(start)
                .take(end.saturating_sub(start))
                .collect(),
            total,
        ))
    }

    async fn list_ended_sessions_pending_pipeline_recovery(
        &self,
        pagination: &Pagination,
    ) -> Result<Vec<LiveSessionDbModel>> {
        let segments = self.segments.lock().expect("lock poisoned");
        let outputs = self.outputs.lock().expect("lock poisoned");
        let dispatched = self
            .session_complete_dispatched
            .lock()
            .expect("lock poisoned");
        let mut sessions = self
            .sessions
            .lock()
            .expect("lock poisoned")
            .values()
            .filter(|session| {
                session.end_time.is_some()
                    && !dispatched.contains(&session.id)
                    && (segments
                        .get(&session.id)
                        .is_some_and(|segments| !segments.is_empty())
                        || outputs
                            .get(&session.id)
                            .is_some_and(|outputs| !outputs.is_empty()))
            })
            .cloned()
            .collect::<Vec<_>>();
        sessions.sort_by_key(|session| Reverse(session.start_time));
        Ok(sessions
            .into_iter()
            .skip(pagination.offset as usize)
            .take(pagination.limit as usize)
            .collect())
    }

    async fn mark_session_complete_dispatched(&self, session_id: &str) -> Result<()> {
        self.session_complete_dispatched
            .lock()
            .expect("lock poisoned")
            .insert(session_id.to_string());
        Ok(())
    }

    async fn get_media_output(&self, _id: &str) -> Result<MediaOutputDbModel> {
        unimplemented!("not needed for these tests")
    }

    async fn get_media_outputs_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<MediaOutputDbModel>> {
        Ok(self
            .outputs
            .lock()
            .expect("lock poisoned")
            .get(session_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn create_media_output(&self, output: &MediaOutputDbModel) -> Result<()> {
        self.insert_output(output.clone());
        Ok(())
    }

    async fn create_segment_output(
        &self,
        output: &MediaOutputDbModel,
        segment: &SessionSegmentDbModel,
    ) -> Result<()> {
        self.insert_output(output.clone());
        self.insert_segment(segment.clone());
        Ok(())
    }

    async fn delete_media_output(&self, _id: &str) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn get_output_count(&self, _session_id: &str) -> Result<u32> {
        unimplemented!("not needed for these tests")
    }

    async fn list_outputs_filtered(
        &self,
        _filters: &OutputFilters,
        _pagination: &Pagination,
    ) -> Result<(Vec<MediaOutputDbModel>, u64)> {
        unimplemented!("not needed for these tests")
    }

    async fn summarize_outputs_filtered(
        &self,
        _filters: &OutputFilters,
    ) -> Result<Vec<MediaOutputTypeSummary>> {
        unimplemented!("not needed for these tests")
    }

    async fn create_session_segment(&self, segment: &SessionSegmentDbModel) -> Result<()> {
        self.insert_segment(segment.clone());
        Ok(())
    }

    async fn list_session_segments_for_session(
        &self,
        session_id: &str,
        limit: i32,
    ) -> Result<Vec<SessionSegmentDbModel>> {
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        Ok(self
            .segments
            .lock()
            .expect("lock poisoned")
            .get(session_id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .take(limit)
            .collect())
    }

    async fn list_session_segments_page(
        &self,
        session_id: &str,
        pagination: &Pagination,
    ) -> Result<Vec<SessionSegmentDbModel>> {
        let start = pagination.offset as usize;
        Ok(self
            .segments
            .lock()
            .expect("lock poisoned")
            .get(session_id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .skip(start)
            .take(pagination.limit as usize)
            .collect())
    }

    async fn next_session_segment_index(&self, session_id: &str) -> Result<u32> {
        let max_index = self
            .segments
            .lock()
            .expect("lock poisoned")
            .get(session_id)
            .and_then(|segments| segments.iter().map(|segment| segment.segment_index).max());

        let next = max_index
            .and_then(|index| index.checked_add(1))
            .unwrap_or(0);

        u32::try_from(next).map_err(|_| {
            crate::Error::Database(format!(
                "next session segment index {} for session {} is outside u32 range",
                next, session_id
            ))
        })
    }

    async fn get_danmu_statistics(
        &self,
        _session_id: &str,
    ) -> Result<Option<DanmuStatisticsDbModel>> {
        unimplemented!("not needed for these tests")
    }

    async fn create_danmu_statistics(&self, _stats: &DanmuStatisticsDbModel) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn upsert_danmu_statistics(&self, _stats: &DanmuStatisticsDbModel) -> Result<()> {
        unimplemented!("not needed for these tests")
    }
}

type BeforePublishObserver = Box<dyn Fn(&DagExecutionDbModel) -> Result<()> + Send + Sync>;

struct TestDagRepository {
    dags: Mutex<HashMap<String, DagExecutionDbModel>>,
    steps: Mutex<HashMap<String, Vec<DagStepExecutionDbModel>>>,
    create_calls: AtomicUsize,
    /// `get_dag` calls left before every row reads back as missing, so a test can model a
    /// `DagScheduler::delete_dag` landing between two reads. `usize::MAX` never runs out.
    reads_before_dags_vanish: AtomicUsize,
    before_publish: Option<BeforePublishObserver>,
}

impl TestDagRepository {
    fn new() -> Self {
        Self {
            dags: Mutex::new(HashMap::new()),
            steps: Mutex::new(HashMap::new()),
            create_calls: AtomicUsize::new(0),
            reads_before_dags_vanish: AtomicUsize::new(usize::MAX),
            before_publish: None,
        }
    }

    fn insert(&self, dag: DagExecutionDbModel) {
        self.dags
            .lock()
            .expect("lock poisoned")
            .insert(dag.id.clone(), dag);
    }

    fn create_calls(&self) -> usize {
        self.create_calls.load(Ordering::SeqCst)
    }

    /// Let the next `reads` calls to `get_dag` succeed, then report every id as missing.
    fn vanish_dags_after_reads(&self, reads: usize) {
        self.reads_before_dags_vanish.store(reads, Ordering::SeqCst);
    }
}

struct TestJobRepository {
    jobs: Mutex<HashMap<String, JobDbModel>>,
}

impl TestJobRepository {
    fn new() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
        }
    }

    fn insert(&self, job: JobDbModel) {
        self.jobs
            .lock()
            .expect("lock poisoned")
            .insert(job.id.clone(), job);
    }

    fn set_status(&self, id: &str, status: JobStatus) {
        self.jobs
            .lock()
            .expect("lock poisoned")
            .get_mut(id)
            .expect("job should exist")
            .status = status.as_str().to_string();
    }

    /// Apply the subset of `JobFilters` these tests exercise: `status`,
    /// `statuses` and `session_id`.
    fn matching(&self, filters: &crate::database::models::JobFilters) -> Vec<JobDbModel> {
        self.jobs
            .lock()
            .expect("lock poisoned")
            .values()
            .filter(|job| {
                filters
                    .status
                    .is_none_or(|status| JobStatus::parse(&job.status) == Some(status))
            })
            .filter(|job| {
                filters.statuses.as_ref().is_none_or(|statuses| {
                    JobStatus::parse(&job.status).is_some_and(|status| statuses.contains(&status))
                })
            })
            .filter(|job| {
                filters
                    .session_id
                    .as_deref()
                    .is_none_or(|session_id| job.session_id.as_deref() == Some(session_id))
            })
            .cloned()
            .collect()
    }
}

#[async_trait]
impl crate::database::repositories::JobRepository for TestJobRepository {
    async fn get_job(&self, id: &str) -> Result<JobDbModel> {
        self.jobs
            .lock()
            .expect("lock poisoned")
            .get(id)
            .cloned()
            .ok_or_else(|| crate::Error::not_found("Job", id))
    }

    async fn list_pending_jobs(&self, _job_type: &str) -> Result<Vec<JobDbModel>> {
        unimplemented!("not needed for these tests")
    }

    async fn list_jobs_by_status(&self, _status: JobStatus) -> Result<Vec<JobDbModel>> {
        unimplemented!("not needed for these tests")
    }

    async fn list_recent_jobs(&self, _limit: i32) -> Result<Vec<JobDbModel>> {
        unimplemented!("not needed for these tests")
    }

    async fn create_job(&self, job: &JobDbModel) -> Result<()> {
        self.insert(job.clone());
        Ok(())
    }

    async fn update_job_status(&self, _id: &str, _status: JobStatus) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn mark_job_failed(
        &self,
        id: &str,
        error: &str,
        retry_after: Option<i64>,
    ) -> Result<u64> {
        let mut jobs = self.jobs.lock().expect("lock poisoned");
        let job = jobs
            .get_mut(id)
            .ok_or_else(|| crate::Error::not_found("Job", id))?;
        if !matches!(
            JobStatus::parse(&job.status),
            Some(JobStatus::Pending | JobStatus::Processing)
        ) {
            return Ok(0);
        }

        let now = chrono::Utc::now().timestamp_millis();
        job.status = JobStatus::Failed.as_str().to_string();
        job.error = Some(error.to_string());
        job.retry_after = retry_after;
        job.completed_at = Some(now);
        job.updated_at = now;
        Ok(1)
    }

    async fn mark_job_cancelled(&self, id: &str) -> Result<u64> {
        let mut jobs = self.jobs.lock().expect("lock poisoned");
        let job = jobs
            .get_mut(id)
            .ok_or_else(|| crate::Error::not_found("Job", id))?;
        if !matches!(
            JobStatus::parse(&job.status),
            Some(JobStatus::Pending | JobStatus::Processing)
        ) {
            return Ok(0);
        }

        let now = chrono::Utc::now().timestamp_millis();
        job.status = JobStatus::Cancelled.as_str().to_string();
        job.completed_at = Some(now);
        job.updated_at = now;
        Ok(1)
    }

    async fn reset_job_for_retry(&self, id: &str) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        let mut jobs = self.jobs.lock().expect("lock poisoned");
        let job = jobs
            .get_mut(id)
            .ok_or_else(|| crate::Error::not_found("Job", id))?;

        match JobStatus::parse(&job.status) {
            Some(JobStatus::Failed | JobStatus::Cancelled) => {
                job.status = JobStatus::Pending.as_str().to_string();
                job.started_at = None;
                job.completed_at = None;
                job.error = None;
                job.retry_count += 1;
                job.updated_at = now;
                Ok(())
            }
            _ => Err(crate::Error::InvalidStateTransition {
                from: job.status.to_ascii_uppercase(),
                to: JobStatus::Pending.as_str().to_string(),
            }),
        }
    }

    async fn count_pending_jobs(&self, _job_types: Option<&[String]>) -> Result<u64> {
        unimplemented!("not needed for these tests")
    }

    async fn count_pending_jobs_by_type(&self) -> Result<Vec<(String, u64)>> {
        unimplemented!("not needed for these tests")
    }

    async fn claim_next_pending_job(
        &self,
        _job_types: Option<&[String]>,
    ) -> Result<Option<JobDbModel>> {
        unimplemented!("not needed for these tests")
    }

    async fn get_job_execution_info(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .jobs
            .lock()
            .expect("lock poisoned")
            .get(id)
            .and_then(|job| job.execution_info.clone()))
    }

    async fn update_job_execution_info(&self, id: &str, execution_info: &str) -> Result<()> {
        let mut jobs = self.jobs.lock().expect("lock poisoned");
        let job = jobs
            .get_mut(id)
            .ok_or_else(|| crate::Error::not_found("Job", id))?;
        job.execution_info = Some(execution_info.to_string());
        Ok(())
    }

    async fn update_job_state(&self, _id: &str, _state: &str) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn update_job(&self, _job: &JobDbModel) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn update_job_if_status(
        &self,
        _job: &JobDbModel,
        _expected_status: JobStatus,
    ) -> Result<u64> {
        unimplemented!("not needed for these tests")
    }

    async fn reset_processing_jobs(&self) -> Result<i32> {
        unimplemented!("not needed for these tests")
    }

    async fn delete_job(&self, _id: &str) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn add_execution_log(&self, _log: &JobExecutionLogDbModel) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    /// Accepted and dropped: terminal transitions persist a log line through this, and no
    /// test here asserts on stored logs.
    async fn add_execution_logs(&self, _logs: &[JobExecutionLogDbModel]) -> Result<()> {
        Ok(())
    }

    async fn get_execution_logs(&self, _job_id: &str) -> Result<Vec<JobExecutionLogDbModel>> {
        unimplemented!("not needed for these tests")
    }

    async fn list_execution_logs(
        &self,
        _job_id: &str,
        _pagination: &crate::database::models::Pagination,
    ) -> Result<(Vec<JobExecutionLogDbModel>, u64)> {
        Ok((Vec::new(), 0))
    }

    async fn delete_execution_logs_for_job(&self, _job_id: &str) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn list_jobs_filtered(
        &self,
        filters: &crate::database::models::JobFilters,
        _pagination: &crate::database::models::Pagination,
    ) -> Result<(Vec<JobDbModel>, u64)> {
        let jobs = self.matching(filters);
        let total = jobs.len() as u64;
        Ok((jobs, total))
    }

    async fn count_jobs(&self, filters: &crate::database::models::JobFilters) -> Result<u64> {
        Ok(self.matching(filters).len() as u64)
    }

    async fn list_jobs_page_filtered(
        &self,
        _filters: &crate::database::models::JobFilters,
        _pagination: &crate::database::models::Pagination,
    ) -> Result<Vec<JobDbModel>> {
        unimplemented!("not needed for these tests")
    }

    async fn get_job_counts_by_status(&self) -> Result<crate::database::models::JobCounts> {
        unimplemented!("not needed for these tests")
    }

    async fn get_avg_processing_time(&self) -> Result<Option<f64>> {
        unimplemented!("not needed for these tests")
    }

    async fn cancel_jobs_by_pipeline(&self, pipeline_id: &str) -> Result<u64> {
        let now = chrono::Utc::now().timestamp_millis();
        let mut cancelled = 0;
        for job in self.jobs.lock().expect("lock poisoned").values_mut() {
            if job.pipeline_id.as_deref() != Some(pipeline_id)
                || !matches!(
                    JobStatus::parse(&job.status),
                    Some(JobStatus::Pending | JobStatus::Processing)
                )
            {
                continue;
            }
            job.status = JobStatus::Cancelled.as_str().to_string();
            job.completed_at = Some(now);
            cancelled += 1;
        }
        Ok(cancelled)
    }

    async fn get_jobs_by_pipeline(&self, pipeline_id: &str) -> Result<Vec<JobDbModel>> {
        Ok(self
            .jobs
            .lock()
            .expect("lock poisoned")
            .values()
            .filter(|job| job.pipeline_id.as_deref() == Some(pipeline_id))
            .cloned()
            .collect())
    }

    async fn delete_jobs_by_pipeline(&self, _pipeline_id: &str) -> Result<u64> {
        unimplemented!("not needed for these tests")
    }
}

struct TestDagRepositoryForRetry {
    dags: Mutex<HashMap<String, DagExecutionDbModel>>,
    steps: HashMap<String, DagStepExecutionDbModel>,
    cancel_calls: AtomicUsize,
}

impl TestDagRepositoryForRetry {
    fn new() -> Self {
        Self {
            dags: Mutex::new(HashMap::new()),
            steps: HashMap::new(),
            cancel_calls: AtomicUsize::new(0),
        }
    }

    fn insert(&self, dag: DagExecutionDbModel) {
        self.dags
            .lock()
            .expect("lock poisoned")
            .insert(dag.id.clone(), dag);
    }

    fn with_step(mut self, step: DagStepExecutionDbModel) -> Self {
        self.steps.insert(step.id.clone(), step);
        self
    }

    fn cancel_calls(&self) -> usize {
        self.cancel_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl DagRepository for TestDagRepositoryForRetry {
    async fn create_dag(&self, _dag: &DagExecutionDbModel) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn publish_dag(
        &self,
        _dag: &DagExecutionDbModel,
        _steps: &[DagStepExecutionDbModel],
        _root_jobs: &[JobDbModel],
    ) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn get_dag(&self, id: &str) -> Result<DagExecutionDbModel> {
        self.dags
            .lock()
            .expect("lock poisoned")
            .get(id)
            .cloned()
            .ok_or_else(|| crate::Error::not_found("DAG execution", id))
    }

    async fn update_dag_status(
        &self,
        _id: &str,
        _status: &str,
        _error: Option<&str>,
    ) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn list_dags(
        &self,
        _status: Option<&str>,
        _session_id: Option<&str>,
        _limit: u32,
        _offset: u32,
    ) -> Result<Vec<DagExecutionDbModel>> {
        unimplemented!("not needed for these tests")
    }

    async fn count_dags(&self, _status: Option<&str>, _session_id: Option<&str>) -> Result<u64> {
        unimplemented!("not needed for these tests")
    }

    async fn list_dag_summaries(
        &self,
        _filter: &crate::database::repositories::DagListFilter<'_>,
        _limit: u32,
        _offset: u32,
    ) -> Result<Vec<DagExecutionListSummary>> {
        unimplemented!("not needed for these tests")
    }

    async fn count_dags_by_display_status(
        &self,
        _filter: &crate::database::repositories::DagListFilter<'_>,
    ) -> Result<u64> {
        unimplemented!("not needed for these tests")
    }

    async fn get_dag_display_status(&self, _id: &str) -> Result<DagExecutionStatus> {
        unimplemented!("not needed for these tests")
    }

    async fn get_dag_with_display_status(
        &self,
        _id: &str,
    ) -> Result<(DagExecutionDbModel, DagExecutionStatus)> {
        unimplemented!("not needed for these tests")
    }

    async fn get_dag_display_counts(&self) -> Result<crate::database::models::DagDisplayCounts> {
        unimplemented!("not needed for these tests")
    }

    async fn delete_dag(&self, _id: &str) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn create_step(&self, _step: &DagStepExecutionDbModel) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn create_steps(&self, _steps: &[DagStepExecutionDbModel]) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn get_step(&self, id: &str) -> Result<DagStepExecutionDbModel> {
        self.steps
            .get(id)
            .cloned()
            .ok_or_else(|| crate::Error::not_found("DAG step execution", id))
    }

    async fn get_step_by_dag_and_step_id(
        &self,
        _dag_id: &str,
        _step_id: &str,
    ) -> Result<DagStepExecutionDbModel> {
        unimplemented!("not needed for these tests")
    }

    async fn get_steps_by_dag(&self, _dag_id: &str) -> Result<Vec<DagStepExecutionDbModel>> {
        Ok(Vec::new())
    }

    async fn get_steps_with_jobs_by_dag(&self, _dag_id: &str) -> Result<Vec<DagStepWithJob>> {
        Ok(Vec::new())
    }

    async fn update_step(&self, _step: &DagStepExecutionDbModel) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn update_step_status(&self, _id: &str, _status: &str) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn update_step_status_with_job(
        &self,
        _id: &str,
        _status: &str,
        _job_id: &str,
    ) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn create_job_for_step(&self, _step_id: &str, _job: &JobDbModel) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn complete_step_and_check_dependents(
        &self,
        _step_id: &str,
        _outputs: &[String],
    ) -> Result<crate::database::repositories::StepCompletion> {
        unimplemented!("not needed for these tests")
    }

    async fn fail_step_and_cancel_dag(
        &self,
        _step_id: &str,
        _error: &str,
    ) -> Result<Option<Vec<String>>> {
        unimplemented!("not needed for these tests")
    }

    async fn fail_dag_and_cancel_steps(&self, _dag_id: &str, _error: &str) -> Result<Vec<String>> {
        unimplemented!("not needed for these tests")
    }

    async fn cancel_dag_and_cancel_steps(&self, dag_id: &str, error: &str) -> Result<Vec<String>> {
        self.cancel_calls.fetch_add(1, Ordering::SeqCst);
        let mut dags = self.dags.lock().expect("lock poisoned");
        let dag = dags
            .get_mut(dag_id)
            .ok_or_else(|| crate::Error::not_found("DAG execution", dag_id))?;
        dag.status = crate::database::models::DagExecutionStatus::Cancelled
            .as_str()
            .to_string();
        dag.completed_at = Some(chrono::Utc::now().timestamp_millis());
        dag.error = Some(error.to_string());
        Ok(Vec::new())
    }

    async fn reset_dag_for_retry(&self, dag_id: &str) -> Result<()> {
        let mut dags = self.dags.lock().expect("lock poisoned");
        let dag = dags
            .get_mut(dag_id)
            .ok_or_else(|| crate::Error::not_found("DAG execution", dag_id))?;
        dag.status = crate::database::models::DagExecutionStatus::Processing
            .as_str()
            .to_string();
        dag.completed_at = None;
        dag.error = None;
        Ok(())
    }

    async fn get_dag_stats(
        &self,
        _dag_id: &str,
    ) -> Result<crate::database::models::DagExecutionStats> {
        unimplemented!("not needed for these tests")
    }

    async fn list_running_step_jobs(&self, _dag_ids: &[&str]) -> Result<Vec<(String, String)>> {
        unimplemented!("not needed for these tests")
    }

    async fn list_ready_steps(&self, _dag_id: &str) -> Result<Vec<ReadyStep>> {
        unimplemented!("not needed for these tests")
    }

    async fn list_processing_steps_with_completed_jobs(
        &self,
    ) -> Result<Vec<DagStepExecutionDbModel>> {
        Ok(Vec::new())
    }

    async fn list_dag_ids_with_unmaterialized_steps(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn list_processing_steps_with_failed_jobs(&self) -> Result<Vec<DagStepExecutionDbModel>> {
        Ok(Vec::new())
    }

    async fn complete_ready_step_without_job(
        &self,
        _step_id: &str,
        _outputs: &[String],
    ) -> Result<crate::database::repositories::StepCompletion> {
        unimplemented!("not needed for these tests")
    }
}

#[async_trait]
impl DagRepository for TestDagRepository {
    async fn create_dag(&self, dag: &DagExecutionDbModel) -> Result<()> {
        self.create_calls.fetch_add(1, Ordering::SeqCst);
        self.insert(dag.clone());
        Ok(())
    }

    async fn publish_dag(
        &self,
        dag: &DagExecutionDbModel,
        steps: &[DagStepExecutionDbModel],
        _root_jobs: &[JobDbModel],
    ) -> Result<()> {
        if let Some(before_publish) = &self.before_publish {
            before_publish(dag)?;
        }
        self.create_dag(dag).await?;
        self.create_steps(steps).await
    }

    async fn get_dag(&self, id: &str) -> Result<DagExecutionDbModel> {
        let remaining = self
            .reads_before_dags_vanish
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                Some(remaining.saturating_sub(1))
            })
            .expect("fetch_update closure always returns Some");
        if remaining == 0 {
            return Err(crate::Error::not_found("DAG execution", id));
        }

        self.dags
            .lock()
            .expect("lock poisoned")
            .get(id)
            .cloned()
            .ok_or_else(|| crate::Error::not_found("DAG execution", id))
    }

    async fn update_dag_status(
        &self,
        _id: &str,
        _status: &str,
        _error: Option<&str>,
    ) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn list_dags(
        &self,
        status: Option<&str>,
        session_id: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<DagExecutionDbModel>> {
        let mut dags = self
            .dags
            .lock()
            .expect("lock poisoned")
            .values()
            .filter(|dag| status.is_none_or(|status| dag.status == status))
            .filter(|dag| {
                session_id.is_none_or(|session_id| dag.session_id.as_deref() == Some(session_id))
            })
            .cloned()
            .collect::<Vec<_>>();

        dags.sort_by_key(|dag| Reverse(dag.created_at));
        Ok(dags
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect())
    }

    async fn count_dags(&self, status: Option<&str>, session_id: Option<&str>) -> Result<u64> {
        Ok(self
            .dags
            .lock()
            .expect("lock poisoned")
            .values()
            .filter(|dag| status.is_none_or(|status| dag.status == status))
            .filter(|dag| {
                session_id.is_none_or(|session_id| dag.session_id.as_deref() == Some(session_id))
            })
            .count() as u64)
    }

    async fn list_dag_summaries(
        &self,
        _filter: &crate::database::repositories::DagListFilter<'_>,
        _limit: u32,
        _offset: u32,
    ) -> Result<Vec<DagExecutionListSummary>> {
        unimplemented!("not needed for these tests")
    }

    async fn count_dags_by_display_status(
        &self,
        _filter: &crate::database::repositories::DagListFilter<'_>,
    ) -> Result<u64> {
        unimplemented!("not needed for these tests")
    }

    async fn get_dag_display_status(&self, _id: &str) -> Result<DagExecutionStatus> {
        unimplemented!("not needed for these tests")
    }

    async fn get_dag_with_display_status(
        &self,
        _id: &str,
    ) -> Result<(DagExecutionDbModel, DagExecutionStatus)> {
        unimplemented!("not needed for these tests")
    }

    async fn get_dag_display_counts(&self) -> Result<crate::database::models::DagDisplayCounts> {
        unimplemented!("not needed for these tests")
    }

    async fn delete_dag(&self, _id: &str) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn create_step(&self, _step: &DagStepExecutionDbModel) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn create_steps(&self, steps: &[DagStepExecutionDbModel]) -> Result<()> {
        let mut by_dag = self.steps.lock().expect("lock poisoned");
        for step in steps {
            by_dag
                .entry(step.dag_id.clone())
                .or_default()
                .push(step.clone());
        }
        Ok(())
    }

    async fn get_step(&self, _id: &str) -> Result<DagStepExecutionDbModel> {
        unimplemented!("not needed for these tests")
    }

    async fn get_step_by_dag_and_step_id(
        &self,
        _dag_id: &str,
        _step_id: &str,
    ) -> Result<DagStepExecutionDbModel> {
        unimplemented!("not needed for these tests")
    }

    async fn get_steps_by_dag(&self, dag_id: &str) -> Result<Vec<DagStepExecutionDbModel>> {
        Ok(self
            .steps
            .lock()
            .expect("lock poisoned")
            .get(dag_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn get_steps_with_jobs_by_dag(&self, _dag_id: &str) -> Result<Vec<DagStepWithJob>> {
        unimplemented!("not needed for these tests")
    }

    async fn update_step(&self, _step: &DagStepExecutionDbModel) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn update_step_status(&self, _id: &str, _status: &str) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn update_step_status_with_job(
        &self,
        _id: &str,
        _status: &str,
        _job_id: &str,
    ) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn create_job_for_step(&self, _step_id: &str, _job: &JobDbModel) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn complete_step_and_check_dependents(
        &self,
        _step_id: &str,
        _outputs: &[String],
    ) -> Result<crate::database::repositories::StepCompletion> {
        unimplemented!("not needed for these tests")
    }

    async fn fail_step_and_cancel_dag(
        &self,
        _step_id: &str,
        _error: &str,
    ) -> Result<Option<Vec<String>>> {
        unimplemented!("not needed for these tests")
    }

    async fn fail_dag_and_cancel_steps(&self, _dag_id: &str, _error: &str) -> Result<Vec<String>> {
        unimplemented!("not needed for these tests")
    }

    /// Mirrors the SQL in `SqlxDagRepository::cancel_dag_and_cancel_steps`: a terminal DAG is left
    /// alone and reports no jobs, otherwise every non-terminal step becomes `CANCELLED` and the
    /// job ids of the `PROCESSING` ones are returned for the caller to cancel.
    async fn cancel_dag_and_cancel_steps(&self, dag_id: &str, error: &str) -> Result<Vec<String>> {
        {
            let mut dags = self.dags.lock().expect("lock poisoned");
            let dag = dags
                .get_mut(dag_id)
                .ok_or_else(|| crate::Error::not_found("DAG execution", dag_id))?;
            if dag.get_status().is_some_and(|status| status.is_terminal()) {
                return Ok(Vec::new());
            }
            dag.status = DagExecutionStatus::Cancelled.as_str().to_string();
            dag.completed_at = Some(chrono::Utc::now().timestamp_millis());
            dag.error = Some(error.to_string());
        }

        let mut steps = self.steps.lock().expect("lock poisoned");
        let mut cancelled_job_ids = Vec::new();
        for step in steps.entry(dag_id.to_string()).or_default() {
            let status = step.get_status();
            if status.is_some_and(|status| status.is_terminal()) {
                continue;
            }
            if status == Some(crate::database::models::DagStepStatus::Processing)
                && let Some(job_id) = step.job_id.clone()
            {
                cancelled_job_ids.push(job_id);
            }
            step.status = crate::database::models::DagStepStatus::Cancelled
                .as_str()
                .to_string();
        }

        Ok(cancelled_job_ids)
    }

    async fn reset_dag_for_retry(&self, _dag_id: &str) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn get_dag_stats(
        &self,
        _dag_id: &str,
    ) -> Result<crate::database::models::DagExecutionStats> {
        unimplemented!("not needed for these tests")
    }

    async fn list_running_step_jobs(&self, _dag_ids: &[&str]) -> Result<Vec<(String, String)>> {
        unimplemented!("not needed for these tests")
    }

    async fn list_ready_steps(&self, _dag_id: &str) -> Result<Vec<ReadyStep>> {
        unimplemented!("not needed for these tests")
    }

    async fn list_processing_steps_with_completed_jobs(
        &self,
    ) -> Result<Vec<DagStepExecutionDbModel>> {
        Ok(Vec::new())
    }

    async fn list_dag_ids_with_unmaterialized_steps(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn list_processing_steps_with_failed_jobs(&self) -> Result<Vec<DagStepExecutionDbModel>> {
        Ok(Vec::new())
    }

    async fn complete_ready_step_without_job(
        &self,
        _step_id: &str,
        _outputs: &[String],
    ) -> Result<crate::database::repositories::StepCompletion> {
        unimplemented!("not needed for these tests")
    }
}

/// The session-complete runner stores per-segment pairing on the DAG row and
/// hands the DAG plain media inputs, videos first; nothing is written next to
/// the recordings.
#[tokio::test]
async fn session_complete_pipeline_stores_pairing_on_the_dag_row() {
    use crate::database::repositories::{SqlxDagRepository, SqlxJobRepository};
    use crate::pipeline::{ManifestScope, PipelineInputManifest, SegmentOutput};

    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let jobs = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
    let dags = Arc::new(SqlxDagRepository::new(pool.clone(), pool.clone()));
    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), jobs.clone())
            .with_dag_repository(dags.clone());
    let dir = tempfile::tempdir().unwrap();
    let segment = |index: u32, name: &str| SegmentOutput {
        segment_index: index,
        path: dir.path().join(name),
    };
    let mut outputs = SessionOutputs::new("session".to_string(), "streamer".to_string(), true);
    outputs.video_outputs = vec![segment(1, "1.mp4"), segment(0, "0.mp4")];
    outputs.danmu_outputs = vec![segment(1, "1.xml")];
    let definition = DagPipelineDefinition::new(
        "session",
        vec![crate::database::models::DagStep::new(
            "A",
            PipelineStep::Inline {
                processor: "remux".to_string(),
                config: serde_json::json!({}),
            },
        )],
    );

    // The returned flag also covers the session-repository dispatch mark, which
    // this manager has no repository for; the DAG row is the evidence here.
    manager
        .run_session_complete_pipeline(outputs, definition)
        .await;

    let published = dags.list_dags(None, Some("session"), 10, 0).await.unwrap();
    assert_eq!(published.len(), 1);
    let manifest: PipelineInputManifest =
        serde_json::from_str(published[0].input_manifest.as_deref().unwrap()).unwrap();
    assert_eq!(manifest.scope, ManifestScope::Session);
    assert_eq!(manifest.session_id, "session");
    assert_eq!(manifest.streamer_id, "streamer");
    assert_eq!(
        manifest
            .segments
            .iter()
            .map(|segment| (
                segment.segment_index,
                segment.video.len(),
                segment.danmu.len()
            ))
            .collect::<Vec<_>>(),
        vec![(0, 1, 0), (1, 1, 1)]
    );
    assert_eq!(
        manifest.danmu_for_video(&dir.path().join("1.mp4").to_string_lossy()),
        &[dir.path().join("1.xml").to_string_lossy().into_owned()]
    );
    let root = jobs.get_jobs_by_pipeline(&published[0].id).await.unwrap();
    assert_eq!(root.len(), 1);
    assert_eq!(
        serde_json::from_str::<Vec<String>>(root[0].input.as_deref().unwrap()).unwrap(),
        vec![
            dir.path().join("0.mp4").to_string_lossy().into_owned(),
            dir.path().join("1.mp4").to_string_lossy().into_owned(),
            dir.path().join("1.xml").to_string_lossy().into_owned(),
        ]
    );
    assert!(
        std::fs::read_dir(dir.path()).unwrap().next().is_none(),
        "no manifest file is written next to the recordings"
    );
}

/// A segment workflow that fails, is retried and then succeeds must hand its
/// retried outputs to the coordinator; the failed run's fallback (the raw
/// recording) must not reach the session-complete workflow instead.
#[tokio::test]
async fn retried_segment_dag_completion_replaces_the_failed_runs_fallback() {
    use crate::database::repositories::{SqlxDagRepository, SqlxJobRepository};

    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let jobs = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
    let dags = Arc::new(SqlxDagRepository::new(pool.clone(), pool.clone()));
    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), jobs.clone())
            .with_dag_repository(dags.clone());
    let remux = |id: &str| {
        crate::database::models::DagStep::new(
            id,
            PipelineStep::Inline {
                processor: "remux".to_string(),
                config: serde_json::json!({}),
            },
        )
    };
    let session_id = "session-1".to_string();
    let streamer_id = "streamer-1".to_string();
    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::ConfigureSession {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            danmu_enabled: false,
            segment_pipeline: Some(DagPipelineDefinition::new("segment", vec![remux("A")])),
            paired_segment_pipeline: None,
            session_complete_pipeline: Some(DagPipelineDefinition::new(
                "session",
                vec![remux("S")],
            )),
        });
    let commands = manager.pipeline_coordinator.apply_event_inline(
        PipelineCoordinationEvent::VideoSegmentCompleted {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            segment_index: 0,
            path: PathBuf::from("/rec/0.flv"),
        },
    );
    manager.execute_pipeline_commands(commands).await;
    let segment_dags = dags
        .list_dags(None, Some(&session_id), 10, 0)
        .await
        .unwrap();
    assert_eq!(segment_dags.len(), 1);
    let dag_id = segment_dags[0].id.clone();
    let step = dags.get_steps_by_dag(&dag_id).await.unwrap().remove(0);
    let job_id = step.job_id.clone().unwrap();
    let scheduler = manager.dag_scheduler().unwrap().clone();

    // First attempt fails: the coordinator adopts the raw recording as fallback.
    jobs.mark_job_failed(&job_id, "boom", None).await.unwrap();
    let failed = scheduler.on_job_failed(&step.id, "boom").await.unwrap();
    manager
        .handle_dag_completion(failed.completion.unwrap())
        .await;

    let retried = manager.retry_dag(&dag_id).await.unwrap();
    assert_eq!(retried.retried_steps, 1);
    let completed = scheduler
        .on_job_completed(
            &step.id,
            &["/rec/0.mp4".to_string()],
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let completion = completed.completion.unwrap();
    assert!(completion.succeeded);
    manager.handle_dag_completion(completion).await;

    let mut commands =
        manager
            .pipeline_coordinator
            .apply_event_inline(PipelineCoordinationEvent::SessionEnded {
                session_id: session_id.clone(),
                streamer_id: streamer_id.clone(),
                should_run_session_complete: true,
            });
    commands.extend(manager.pipeline_coordinator.apply_event_inline(
        PipelineCoordinationEvent::SessionEndPersisted {
            session_id: session_id.clone(),
        },
    ));
    manager.execute_pipeline_commands(commands).await;

    let session_dag = dags
        .list_dags(None, Some(&session_id), 10, 0)
        .await
        .unwrap()
        .into_iter()
        .find(|dag| dag.segment_source.as_deref() == Some("session_complete"))
        .expect("the session-complete workflow was published");
    let root = jobs.get_jobs_by_pipeline(&session_dag.id).await.unwrap();
    assert_eq!(root.len(), 1);
    assert_eq!(
        serde_json::from_str::<Vec<String>>(root[0].input.as_deref().unwrap()).unwrap(),
        vec!["/rec/0.mp4".to_string()],
        "the retried output, not the failed run's fallback, reaches session-complete"
    );
}

async fn sqlx_manager() -> (
    PipelineManager,
    Arc<crate::database::repositories::SqlxJobRepository>,
    Arc<crate::database::repositories::SqlxDagRepository>,
    sqlx::SqlitePool,
) {
    use crate::database::repositories::{SqlxDagRepository, SqlxJobRepository};
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let jobs = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
    let dags = Arc::new(SqlxDagRepository::new(pool.clone(), pool.clone()));
    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), jobs.clone())
            .with_dag_repository(dags.clone());
    (manager, jobs, dags, pool)
}

/// A→{B,C}→D of inline `remux` steps.
fn remux_diamond(name: &str) -> DagPipelineDefinition {
    let remux = || PipelineStep::Inline {
        processor: "remux".to_string(),
        config: serde_json::json!({}),
    };
    DagPipelineDefinition::new(
        name,
        vec![
            crate::database::models::DagStep::new("A", remux()),
            crate::database::models::DagStep::with_dependencies(
                "B",
                remux(),
                vec!["A".to_string()],
            ),
            crate::database::models::DagStep::with_dependencies(
                "C",
                remux(),
                vec!["A".to_string()],
            ),
            crate::database::models::DagStep::with_dependencies(
                "D",
                remux(),
                vec!["B".to_string(), "C".to_string()],
            ),
        ],
    )
}

fn remux_chain(name: &str, ids: &[&str]) -> DagPipelineDefinition {
    DagPipelineDefinition::new(
        name,
        ids.iter()
            .enumerate()
            .map(|(index, id)| {
                let step = PipelineStep::Inline {
                    processor: "remux".to_string(),
                    config: serde_json::json!({}),
                };
                if index == 0 {
                    crate::database::models::DagStep::new(*id, step)
                } else {
                    crate::database::models::DagStep::with_dependencies(
                        *id,
                        step,
                        vec![ids[index - 1].to_string()],
                    )
                }
            })
            .collect(),
    )
}

/// Retry checks every job it would restart before changing a row: a job that
/// cannot be restarted is reported while the DAG is still terminal and
/// retryable.
#[tokio::test]
async fn retry_dag_rejects_an_unrestartable_job_before_touching_rows() {
    let (manager, jobs, dags, pool) = sqlx_manager().await;
    let created = manager
        .create_dag_pipeline(
            "session",
            "streamer",
            vec!["/in.flv".to_string()],
            remux_chain("missing job", &["A"]),
        )
        .await
        .unwrap();
    let scheduler = manager.dag_scheduler().unwrap().clone();
    let step = dags
        .get_steps_by_dag(&created.dag_id)
        .await
        .unwrap()
        .remove(0);
    jobs.mark_job_failed(&created.root_job_ids[0], "boom", None)
        .await
        .unwrap();
    scheduler.on_job_failed(&step.id, "boom").await.unwrap();
    // A worker still owns the job: the row reads PROCESSING although the step
    // already failed (the failure report raced a crash-recovery reset).
    sqlx::query("UPDATE job SET status = 'PROCESSING' WHERE id = ?")
        .bind(&created.root_job_ids[0])
        .execute(&pool)
        .await
        .unwrap();

    let error = manager.retry_dag(&created.dag_id).await.unwrap_err();
    assert!(matches!(error, Error::Validation(_)));
    assert!(
        error
            .to_string()
            .contains("is PROCESSING and cannot be retried"),
        "{error}"
    );
    let dag = dags.get_dag(&created.dag_id).await.unwrap();
    assert_eq!(dag.status, "FAILED");
    assert_eq!(dag.error.as_deref(), Some("Step 'A' failed: boom"));
    assert_eq!(
        dags.get_step(&step.id).await.unwrap().status,
        "FAILED",
        "rows are untouched by a rejected retry"
    );
}

/// A DAG whose only cancelled step never received a job (its creation failed)
/// is retryable: the reset unblocks the step and materializes it.
#[tokio::test]
async fn retry_dag_materializes_a_cancelled_step_without_a_job() {
    let (manager, jobs, dags, pool) = sqlx_manager().await;
    let created = manager
        .create_dag_pipeline(
            "session",
            "streamer",
            vec!["/in.flv".to_string()],
            remux_chain("jobless cancel", &["A", "B"]),
        )
        .await
        .unwrap();
    let scheduler = manager.dag_scheduler().unwrap().clone();
    let steps = dags.get_steps_by_dag(&created.dag_id).await.unwrap();
    let step_a = steps.iter().find(|step| step.step_id == "A").unwrap();
    let step_b = steps.iter().find(|step| step.step_id == "B").unwrap();
    let update = scheduler
        .on_job_completed(&step_a.id, &["/a.mp4".to_string()], None, None, None, None)
        .await
        .unwrap();
    // The shape a failed downstream job creation leaves behind.
    jobs.delete_job(&update.new_job_ids[0]).await.unwrap();
    sqlx::query("UPDATE dag_step_execution SET status = 'CANCELLED', job_id = NULL WHERE id = ?")
        .bind(&step_b.id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE dag_execution SET status = 'FAILED', error = 'creation failed' WHERE id = ?",
    )
    .bind(&created.dag_id)
    .execute(&pool)
    .await
    .unwrap();

    let result = manager.retry_dag(&created.dag_id).await.unwrap();
    assert_eq!(result.retried_steps, 1);
    assert_eq!(result.job_ids.len(), 1);
    let dag = dags.get_dag(&created.dag_id).await.unwrap();
    assert_eq!(dag.status, "PROCESSING");
    let step_b = dags.get_step(&step_b.id).await.unwrap();
    assert_eq!(step_b.status, "PROCESSING");
    assert_eq!(step_b.job_id.as_deref(), Some(result.job_ids[0].as_str()));
    assert_eq!(
        serde_json::from_str::<Vec<String>>(
            jobs.get_job(&result.job_ids[0])
                .await
                .unwrap()
                .input
                .as_deref()
                .unwrap()
        )
        .unwrap(),
        vec!["/a.mp4".to_string()]
    );
}

/// After a partial failure only the failed branch is re-run: the failed step's
/// job is restarted, its cancelled dependent is materialized once that step
/// completes again, and the branch that finished keeps its outputs.
#[tokio::test]
async fn retry_dag_after_a_partial_failure_reruns_only_the_failed_branch() {
    let (manager, jobs, dags, _pool) = sqlx_manager().await;
    let created = manager
        .create_dag_pipeline(
            "session",
            "streamer",
            vec!["/in.flv".to_string()],
            remux_diamond("partial failure"),
        )
        .await
        .unwrap();
    let scheduler = manager.dag_scheduler().unwrap().clone();
    let steps = dags.get_steps_by_dag(&created.dag_id).await.unwrap();
    let step_id = |id: &str| {
        steps
            .iter()
            .find(|step| step.step_id == id)
            .unwrap()
            .id
            .clone()
    };
    let statuses = |dags: Arc<crate::database::repositories::SqlxDagRepository>| {
        let dag_id = created.dag_id.clone();
        async move {
            let mut statuses: Vec<String> = dags
                .get_steps_by_dag(&dag_id)
                .await
                .unwrap()
                .into_iter()
                .map(|step| format!("{}={}", step.step_id, step.status))
                .collect();
            statuses.sort();
            statuses
        }
    };

    let update = scheduler
        .on_job_completed(
            &step_id("A"),
            &["/a.mp4".to_string()],
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(update.new_job_ids.len(), 2);
    let job_b = dags.get_step(&step_id("B")).await.unwrap().job_id.unwrap();
    // The worker fails the job before it reports the step failure.
    assert_eq!(jobs.mark_job_failed(&job_b, "boom", None).await.unwrap(), 1);
    let update = scheduler
        .on_job_failed(&step_id("B"), "boom")
        .await
        .unwrap();
    assert!(update.completion.is_none(), "C is still running");
    let update = scheduler
        .on_job_completed(
            &step_id("C"),
            &["/c.mp4".to_string()],
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert!(
        update
            .completion
            .is_some_and(|completion| !completion.succeeded)
    );
    assert_eq!(
        dags.get_dag(&created.dag_id).await.unwrap().status,
        "FAILED"
    );
    assert_eq!(
        statuses(dags.clone()).await,
        ["A=COMPLETED", "B=FAILED", "C=COMPLETED", "D=CANCELLED"]
    );

    let result = manager.retry_dag(&created.dag_id).await.unwrap();
    assert_eq!(result.retried_steps, 2, "B and D");
    assert_eq!(result.job_ids, vec![job_b.clone()], "only B's job restarts");
    let dag = dags.get_dag(&created.dag_id).await.unwrap();
    assert_eq!(dag.status, "PROCESSING");
    assert!(dag.error.is_none());
    assert_eq!(dag.failed_steps, 0);
    assert_eq!(jobs.get_job(&job_b).await.unwrap().status, "PENDING");
    assert_eq!(
        statuses(dags.clone()).await,
        ["A=COMPLETED", "B=PROCESSING", "C=COMPLETED", "D=BLOCKED"]
    );
    assert!(dags.get_step(&step_id("D")).await.unwrap().job_id.is_none());
    assert_eq!(
        dags.get_step(&step_id("C")).await.unwrap().get_outputs(),
        vec!["/c.mp4".to_string()],
        "the finished branch keeps its outputs"
    );

    // B succeeds this time: D receives B's and C's outputs in dependency order.
    let update = scheduler
        .on_job_completed(
            &step_id("B"),
            &["/b.mp4".to_string()],
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(update.new_job_ids.len(), 1);
    assert!(update.completion.is_none());
    let job_d = jobs.get_job(&update.new_job_ids[0]).await.unwrap();
    assert_eq!(
        serde_json::from_str::<Vec<String>>(job_d.input.as_deref().unwrap()).unwrap(),
        vec!["/b.mp4".to_string(), "/c.mp4".to_string()]
    );
    assert_eq!(
        statuses(dags.clone()).await,
        ["A=COMPLETED", "B=COMPLETED", "C=COMPLETED", "D=PROCESSING"]
    );
    assert_eq!(
        jobs.get_jobs_by_pipeline(&created.dag_id)
            .await
            .unwrap()
            .len(),
        4,
        "B's job was reused"
    );
}

/// The retry sweeper re-queues a due retry whose step still waits on the job
/// and drops one whose workflow has settled since.
#[tokio::test]
async fn run_due_retries_requeues_live_attempts_and_drops_settled_ones() {
    let (manager, jobs, _dags, _pool) = sqlx_manager().await;
    let live = manager
        .create_dag_pipeline(
            "session",
            "streamer",
            vec!["/in.flv".to_string()],
            remux_chain("live retry", &["A"]),
        )
        .await
        .unwrap();
    let settled = manager
        .create_dag_pipeline(
            "session",
            "streamer",
            vec!["/in.flv".to_string()],
            remux_chain("settled retry", &["A"]),
        )
        .await
        .unwrap();
    for created in [&live, &settled] {
        let job_id = &created.root_job_ids[0];
        assert_eq!(
            jobs.mark_job_failed(job_id, "boom", Some(0)).await.unwrap(),
            1
        );
    }
    manager.cancel_pipeline(&settled.dag_id).await.unwrap();

    assert_eq!(manager.run_due_retries().await, 1);
    let live_job = jobs.get_job(&live.root_job_ids[0]).await.unwrap();
    assert_eq!(live_job.status, "PENDING");
    assert_eq!(live_job.retry_count, 1);
    assert!(live_job.retry_after.is_none());
    let settled_job = jobs.get_job(&settled.root_job_ids[0]).await.unwrap();
    assert_eq!(settled_job.status, "FAILED");
    assert!(
        settled_job.retry_after.is_none(),
        "a retry of a cancelled workflow is dropped"
    );
    assert_eq!(manager.run_due_retries().await, 0);
}

#[tokio::test]
async fn test_cancel_dag_step_job_resolves_parent_before_mutating_job() {
    let job_repo = Arc::new(TestJobRepository::new());
    let dag_repo = Arc::new(TestDagRepositoryForRetry::new());

    let mut job = JobDbModel::new_pipeline_step("remux", "[]", "[]", 0, None, None);
    job.dag_step_execution_id = Some("missing-step-exec".to_string());
    let job_id = job.id.clone();
    job_repo.insert(job);

    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), job_repo)
            .with_dag_repository(dag_repo);

    let error = manager.cancel_job(&job_id).await.unwrap_err();

    assert!(matches!(error, crate::Error::NotFound { .. }));
    assert_eq!(
        manager.get_job(&job_id).await.unwrap().unwrap().status,
        JobStatus::Pending
    );
}

#[tokio::test]
async fn test_cancel_dag_step_job_allows_terminal_parent() {
    let job_repo = Arc::new(TestJobRepository::new());

    let mut dag = DagExecutionDbModel::new(
        &crate::database::models::DagPipelineDefinition::new("test", Vec::new()),
        None,
        None,
    );
    dag.status = crate::database::models::DagExecutionStatus::Failed
        .as_str()
        .to_string();
    let dag_id = dag.id.clone();

    let mut step = DagStepExecutionDbModel::new(&dag_id, "step-a", &[]);
    let step_id = step.id.clone();
    step.status = crate::database::models::DagStepStatus::Failed
        .as_str()
        .to_string();
    let dag_repo = Arc::new(TestDagRepositoryForRetry::new().with_step(step));
    dag_repo.insert(dag);

    let mut job = JobDbModel::new_pipeline_step("remux", "[]", "[]", 0, None, None);
    job.dag_step_execution_id = Some(step_id);
    let job_id = job.id.clone();
    job_repo.insert(job);

    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), job_repo)
            .with_dag_repository(dag_repo.clone());

    manager.cancel_job(&job_id).await.unwrap();

    assert_eq!(
        manager.get_job(&job_id).await.unwrap().unwrap().status,
        JobStatus::Cancelled
    );
    assert_eq!(dag_repo.cancel_calls(), 0);
}

/// `cancel_pipeline` on a DAG id must stand the whole DAG down, not just its jobs: the
/// `dag_execution` row and its steps have to reach a terminal state and the coordinator has to be
/// told the segment DAG is over, otherwise session-complete waits on it forever.
#[tokio::test]
async fn test_cancel_pipeline_cancels_dag_and_releases_session_complete() {
    let session_repo = Arc::new(TestSessionRepository::new(Some(
        chrono::Utc::now().timestamp_millis(),
    )));
    let dag_repo = Arc::new(TestDagRepository::new());
    let job_repo = Arc::new(TestJobRepository::new());
    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), job_repo.clone())
            .with_session_repository(session_repo)
            .with_dag_repository(dag_repo.clone());

    let session_id = "cancel-pipeline-session".to_string();
    let streamer_id = "cancel-pipeline-streamer".to_string();
    let segment_pipeline = DagPipelineDefinition::new(
        "segment",
        vec![DagStep::new(
            "A",
            PipelineStep::inline("remux", serde_json::json!({})),
        )],
    );

    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::ConfigureSession {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            danmu_enabled: false,
            segment_pipeline: Some(segment_pipeline.clone()),
            paired_segment_pipeline: None,
            session_complete_pipeline: Some(DagPipelineDefinition::new(
                "session",
                vec![DagStep::new(
                    "session-step",
                    PipelineStep::inline("remux", serde_json::json!({})),
                )],
            )),
        });
    for segment_index in 0..2 {
        manager.pipeline_coordinator.apply_event_inline(
            PipelineCoordinationEvent::SegmentDagStarted {
                session_id: session_id.clone(),
                streamer_id: streamer_id.clone(),
                segment_index,
                source: SourceType::Video,
            },
        );
    }
    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::SessionEnded {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            should_run_session_complete: true,
        });
    manager.pipeline_coordinator.apply_event_inline(
        PipelineCoordinationEvent::SessionEndPersisted {
            session_id: session_id.clone(),
        },
    );

    // Segment 0 finishes normally: it gives the session the video output that
    // `artifacts_drained_for_session_complete` requires, leaving segment 1 as the only thing
    // session-complete is still waiting on.
    let mut finished_dag = DagExecutionDbModel::new(
        &segment_pipeline,
        Some(streamer_id.clone()),
        Some(session_id.clone()),
    );
    finished_dag.segment_index = Some(0);
    finished_dag.segment_source = Some("video".to_string());
    let finished_dag_id = finished_dag.id.clone();
    dag_repo.insert(finished_dag);
    manager
        .handle_dag_completion(DagCompletionInfo {
            dag_id: finished_dag_id,
            streamer_id: Some(streamer_id.clone()),
            session_id: Some(session_id.clone()),
            succeeded: true,
            leaf_outputs: vec!["/out0.mp4".to_string()],
        })
        .await;
    assert_eq!(
        dag_repo.create_calls(),
        0,
        "session-complete must wait for the second segment DAG"
    );

    let mut dag = DagExecutionDbModel::new(
        &segment_pipeline,
        Some(streamer_id.clone()),
        Some(session_id.clone()),
    );
    dag.status = DagExecutionStatus::Processing.as_str().to_string();
    dag.segment_index = Some(1);
    dag.segment_source = Some("video".to_string());
    let dag_id = dag.id.clone();
    dag_repo.insert(dag);

    let mut step = DagStepExecutionDbModel::new(&dag_id, "A", &[]);
    step.status = crate::database::models::DagStepStatus::Processing
        .as_str()
        .to_string();
    let mut job = JobDbModel::new_pipeline_step(
        "remux",
        "[]",
        "[]",
        0,
        Some(streamer_id.clone()),
        Some(session_id.clone()),
    );
    job.pipeline_id = Some(dag_id.clone());
    job.dag_step_execution_id = Some(step.id.clone());
    let job_id = job.id.clone();
    step.job_id = Some(job_id.clone());
    job_repo.insert(job);
    dag_repo.create_steps(&[step]).await.unwrap();

    let cancelled = manager.cancel_pipeline(&dag_id).await.unwrap();

    assert_eq!(cancelled, 1);
    assert_eq!(
        dag_repo.get_dag(&dag_id).await.unwrap().get_status(),
        Some(DagExecutionStatus::Cancelled)
    );
    assert!(
        dag_repo
            .get_steps_by_dag(&dag_id)
            .await
            .unwrap()
            .iter()
            .all(|step| step.get_status().is_some_and(|status| status.is_terminal())),
        "every step of a cancelled DAG must be terminal so recover_dag_jobs and retry_dag can act on it"
    );
    assert_eq!(
        manager.get_job(&job_id).await.unwrap().unwrap().status,
        JobStatus::Cancelled
    );
    assert_eq!(
        dag_repo.create_calls(),
        1,
        "cancelling the last in-flight segment DAG must release session-complete"
    );
}

/// `cancel_dag` reaches step jobs only through `dag_step_execution.job_id`, so `cancel_pipeline`
/// also sweeps by `pipeline_id`: a job row whose step carries no `job_id` still has to be cancelled,
/// and the jobs the DAG cancel already stood down must not be counted a second time.
#[tokio::test]
async fn test_cancel_pipeline_sweeps_dag_jobs_not_linked_to_a_step() {
    let dag_repo = Arc::new(TestDagRepository::new());
    let job_repo = Arc::new(TestJobRepository::new());

    let mut dag = DagExecutionDbModel::new(
        &DagPipelineDefinition::new(
            "segment",
            vec![
                DagStep::new("A", PipelineStep::inline("remux", serde_json::json!({}))),
                DagStep::new("B", PipelineStep::inline("remux", serde_json::json!({}))),
            ],
        ),
        None,
        None,
    );
    dag.status = DagExecutionStatus::Processing.as_str().to_string();
    let dag_id = dag.id.clone();
    dag_repo.insert(dag);

    // Step A holds the usual `job_id` back-reference; step B does not, so only its job row's
    // `pipeline_id` still ties that job to the DAG.
    let mut linked_step = DagStepExecutionDbModel::new(&dag_id, "A", &[]);
    linked_step.status = crate::database::models::DagStepStatus::Processing
        .as_str()
        .to_string();
    let mut linked_job = JobDbModel::new_pipeline_step("remux", "[]", "[]", 0, None, None);
    linked_job.pipeline_id = Some(dag_id.clone());
    linked_job.dag_step_execution_id = Some(linked_step.id.clone());
    let linked_job_id = linked_job.id.clone();
    linked_step.job_id = Some(linked_job_id.clone());
    job_repo.insert(linked_job);

    let unlinked_step = DagStepExecutionDbModel::new(&dag_id, "B", &[]);
    let mut unlinked_job = JobDbModel::new_pipeline_step("remux", "[]", "[]", 0, None, None);
    unlinked_job.pipeline_id = Some(dag_id.clone());
    unlinked_job.dag_step_execution_id = Some(unlinked_step.id.clone());
    let unlinked_job_id = unlinked_job.id.clone();
    job_repo.insert(unlinked_job);

    dag_repo
        .create_steps(&[linked_step, unlinked_step])
        .await
        .unwrap();

    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), job_repo)
            .with_dag_repository(dag_repo.clone());

    assert_eq!(manager.cancel_pipeline(&dag_id).await.unwrap(), 2);
    assert_eq!(
        manager
            .get_job(&linked_job_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        JobStatus::Cancelled
    );
    assert_eq!(
        manager
            .get_job(&unlinked_job_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        JobStatus::Cancelled
    );
    assert_eq!(
        dag_repo.get_dag(&dag_id).await.unwrap().get_status(),
        Some(DagExecutionStatus::Cancelled)
    );
}

/// A DAG deleted between `get_dag_status` and `cancel_dag` leaves nothing to stand down, so the
/// cancel reports zero jobs rather than surfacing the not-found error.
#[tokio::test]
async fn test_cancel_pipeline_tolerates_dag_deleted_mid_cancel() {
    let dag_repo = Arc::new(TestDagRepository::new());
    let job_repo = Arc::new(TestJobRepository::new());

    let mut dag = DagExecutionDbModel::new(
        &DagPipelineDefinition::new(
            "segment",
            vec![DagStep::new(
                "A",
                PipelineStep::inline("remux", serde_json::json!({})),
            )],
        ),
        None,
        None,
    );
    dag.status = DagExecutionStatus::Processing.as_str().to_string();
    let dag_id = dag.id.clone();
    dag_repo.insert(dag);

    // The row survives the status read in `cancel_pipeline` and is gone by the time
    // `DagScheduler::cancel_dag_with_completion` re-reads it.
    dag_repo.vanish_dags_after_reads(1);

    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), job_repo)
            .with_dag_repository(dag_repo);

    assert_eq!(manager.cancel_pipeline(&dag_id).await.unwrap(), 0);
}

#[tokio::test]
async fn stop_settles_owned_pipeline_runtime_tasks() {
    let manager: Arc<PipelineManager> = Arc::new(PipelineManager::with_repository(
        Default::default(),
        crate::database::test_support::job_repository().await,
    ));
    assert!(manager.job_queue.progress_aggregator_is_running());

    manager.clone().start();
    // Completion handler, coordinator, retry sweeper, stale-state cleanup.
    assert_eq!(manager.runtime.lock().tasks.len(), 4);

    tokio::time::timeout(std::time::Duration::from_secs(1), manager.stop())
        .await
        .expect("pipeline stop should settle every owned runtime task");

    let runtime = manager.runtime.lock();
    assert_eq!(runtime.state, PipelineRuntimeState::Stopped);
    assert!(runtime.tasks.is_empty());
    drop(runtime);
    assert!(!manager.job_queue.progress_aggregator_is_running());
}

#[tokio::test]
async fn baidupcs_upload_jobs_are_claimed() {
    let manager: Arc<PipelineManager> = Arc::new(PipelineManager::with_repository(
        Default::default(),
        crate::database::test_support::job_repository().await,
    ));
    manager.clone().start();

    let job_id = manager
        .enqueue(Job::new("baidupcs", vec![], vec![], "streamer", "session"))
        .await
        .unwrap();
    let job = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let job = manager.get_job(&job_id).await.unwrap().unwrap();
            if job.status == JobStatus::Failed {
                break job;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a worker should claim and run the upload job");
    assert!(
        job.error
            .as_deref()
            .is_some_and(|error| error.contains("No input files provided")),
        "unexpected error: {:?}",
        job.error
    );

    manager.stop().await;
}

/// Every processor name the preset editor can write must be claimable by a pool, or the jobs
/// it produces sit PENDING forever. Mirrors `VALID_PROCESSORS` in
/// `frontend/src/api/schemas/pipeline.ts`; the seeded `archive_zip` job preset in the initial
/// schema migration also relies on "compression" resolving here.
#[tokio::test]
async fn test_frontend_processor_names_are_all_claimable() {
    let manager: PipelineManager = PipelineManager::with_repository(
        Default::default(),
        crate::database::test_support::job_repository().await,
    );
    let supported = manager.supported_job_types();

    for processor in [
        "remux",
        "rclone",
        "baidupcs",
        "thumbnail",
        "execute",
        "audio_extract",
        "compression",
        "copy_move",
        "delete",
        "metadata",
        "danmaku_factory",
        "ass_burnin",
    ] {
        assert!(
            supported.contains(processor),
            "processor '{processor}' is offered by the preset editor but no pool can claim it"
        );
    }
}

fn consuming_step_dag(processor: &str, config: serde_json::Value) -> DagPipelineDefinition {
    DagPipelineDefinition::new(
        "consumer beside a reader",
        vec![
            DagStep::new(
                "remux",
                PipelineStep::inline("remux", serde_json::json!({})),
            ),
            DagStep::with_dependencies(
                "thumbnail",
                PipelineStep::inline("thumbnail", serde_json::json!({})),
                vec!["remux".to_string()],
            ),
            DagStep::with_dependencies(
                "consumer",
                PipelineStep::inline(processor, config),
                vec!["remux".to_string()],
            ),
        ],
    )
}

/// A consuming step that depends on the reader, directly or through other
/// steps, runs after it and is accepted.
#[test]
fn test_validate_consuming_steps_accepts_a_delete_after_its_readers() {
    let after_thumbnail = DagPipelineDefinition::new(
        "delete after thumbnail",
        vec![
            DagStep::new(
                "remux",
                PipelineStep::inline("remux", serde_json::json!({})),
            ),
            DagStep::with_dependencies(
                "thumbnail",
                PipelineStep::inline("thumbnail", serde_json::json!({})),
                vec!["remux".to_string()],
            ),
            DagStep::with_dependencies(
                "delete",
                PipelineStep::inline("delete", serde_json::json!({})),
                vec!["thumbnail".to_string()],
            ),
        ],
    );
    super::dag::validate_consuming_steps(&after_thumbnail).unwrap();

    let fan_in = DagPipelineDefinition::new(
        "delete fans in",
        vec![
            DagStep::new(
                "remux",
                PipelineStep::inline("remux", serde_json::json!({})),
            ),
            DagStep::with_dependencies(
                "thumbnail",
                PipelineStep::inline("thumbnail", serde_json::json!({})),
                vec!["remux".to_string()],
            ),
            DagStep::with_dependencies(
                "upload",
                PipelineStep::inline("rclone", serde_json::json!({ "operation": "copy" })),
                vec!["thumbnail".to_string()],
            ),
            DagStep::with_dependencies(
                "delete",
                PipelineStep::inline("delete", serde_json::json!({})),
                vec!["remux".to_string(), "upload".to_string()],
            ),
        ],
    );
    super::dag::validate_consuming_steps(&fan_in).unwrap();
}

/// Root steps all read the pipeline's inputs, so two destructive roots race
/// each other and are rejected too.
#[test]
fn test_validate_consuming_steps_rejects_two_destructive_roots() {
    let dag = DagPipelineDefinition::new(
        "two destructive roots",
        vec![
            DagStep::new(
                "delete",
                PipelineStep::inline("delete", serde_json::json!({})),
            ),
            DagStep::new(
                "move",
                PipelineStep::inline("copy_move", serde_json::json!({ "operation": "move" })),
            ),
        ],
    );
    let message = super::dag::validate_consuming_steps(&dag)
        .expect_err("both roots consume the pipeline's inputs")
        .to_string();
    assert!(
        message.contains("removes the pipeline's inputs"),
        "{message}"
    );
}

/// Only the modes that remove local files count: rclone and copy/move in
/// `move` mode and a Baidu upload that deletes its sources.
#[test]
fn test_validate_consuming_steps_distinguishes_transfer_modes() {
    for (processor, config) in [
        ("copy_move", serde_json::json!({ "operation": "move" })),
        ("rclone", serde_json::json!({ "operation": "Move" })),
        (
            "baidupcs",
            serde_json::json!({ "remove_source_after_upload": true }),
        ),
    ] {
        super::dag::validate_consuming_steps(&consuming_step_dag(processor, config.clone()))
            .expect_err(&format!("{processor} {config} removes its inputs"));
    }
    for (processor, config) in [
        ("copy_move", serde_json::json!({ "operation": "copy" })),
        ("rclone", serde_json::json!({})),
        (
            "baidupcs",
            serde_json::json!({ "remove_source_after_upload": false }),
        ),
        (
            "metadata",
            serde_json::json!({ "remove_input_on_success": true }),
        ),
    ] {
        super::dag::validate_consuming_steps(&consuming_step_dag(processor, config.clone()))
            .unwrap_or_else(|error| panic!("{processor} {config} only reads its inputs: {error}"));
    }
}

/// Option-driven removal beside a reader is a warning naming the option and
/// the readers; ordering the reader first clears it.
#[test]
fn test_consuming_step_warnings_flag_option_driven_deletion_beside_a_reader() {
    let warnings = super::dag::consuming_step_warnings(&consuming_step_dag(
        "remux",
        serde_json::json!({ "remove_input_on_success": true }),
    ));
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let warning = &warnings[0];
    assert!(
        warning.contains("Step 'consumer' deletes the outputs of 'remux'"),
        "{warning}"
    );
    assert!(warning.contains("(remove_input_on_success)"), "{warning}");
    assert!(warning.contains("'thumbnail' still read them"), "{warning}");
    assert!(warning.contains("depend on that step"), "{warning}");

    for (processor, config) in [
        (
            "ass_burnin",
            serde_json::json!({ "delete_source_videos_on_success": true }),
        ),
        (
            "ass_burnin",
            serde_json::json!({ "delete_source_ass_on_success": true }),
        ),
        (
            "danmaku_factory",
            serde_json::json!({ "delete_source_xml_on_success": true }),
        ),
        (
            "metadata",
            serde_json::json!({ "remove_input_on_success": true }),
        ),
    ] {
        let warnings =
            super::dag::consuming_step_warnings(&consuming_step_dag(processor, config.clone()));
        assert_eq!(warnings.len(), 1, "{processor} {config}: {warnings:?}");
    }
    for (processor, config) in [
        (
            "remux",
            serde_json::json!({ "remove_input_on_success": false }),
        ),
        ("remux", serde_json::json!({})),
        ("thumbnail", serde_json::json!({})),
        ("delete", serde_json::json!({})),
    ] {
        let warnings =
            super::dag::consuming_step_warnings(&consuming_step_dag(processor, config.clone()));
        assert!(warnings.is_empty(), "{processor} {config}: {warnings:?}");
    }

    // The seeded "Stream Archive" shape: the thumbnail depends on the
    // deleting remux, so it always reads first.
    let ordered = DagPipelineDefinition::new(
        "ordered",
        vec![
            DagStep::new(
                "remux",
                PipelineStep::inline(
                    "remux",
                    serde_json::json!({ "remove_input_on_success": true }),
                ),
            ),
            DagStep::with_dependencies(
                "thumbnail",
                PipelineStep::inline("thumbnail", serde_json::json!({})),
                vec!["remux".to_string()],
            ),
        ],
    );
    assert!(super::dag::consuming_step_warnings(&ordered).is_empty());
}

/// The check runs when a DAG is created, so the racing definition never
/// reaches the scheduler.
#[tokio::test]
async fn test_create_dag_pipeline_rejects_a_step_that_removes_a_siblings_inputs() {
    let (manager, _jobs, dags, _pool) = sqlx_manager().await;
    let error = manager
        .create_dag_pipeline(
            "session",
            "streamer",
            vec!["/in.flv".to_string()],
            consuming_step_dag("delete", serde_json::json!({})),
        )
        .await
        .expect_err("rejected before publication");
    assert!(matches!(error, Error::Validation(_)), "{error}");
    assert!(
        dags.list_dag_ids_with_unmaterialized_steps()
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn test_paired_segment_recovers_segment_dag_completion_without_context() {
    let dag_repo = Arc::new(TestDagRepository::new());
    let manager: PipelineManager = PipelineManager::with_repository(
        Default::default(),
        crate::database::test_support::job_repository().await,
    )
    .with_dag_repository(dag_repo.clone());

    let session_id = "session-1".to_string();
    let streamer_id = "streamer-1".to_string();

    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::ConfigureSession {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            danmu_enabled: true,
            segment_pipeline: None,
            paired_segment_pipeline: Some(DagPipelineDefinition::new(
                "paired",
                vec![DagStep::new("A", PipelineStep::preset("remux"))],
            )),
            session_complete_pipeline: None,
        });

    let dag_def = DagPipelineDefinition::new(
        "segment-dag",
        vec![DagStep::new("A", PipelineStep::preset("remux"))],
    );

    let mut video_dag = DagExecutionDbModel::new(
        &dag_def,
        Some(streamer_id.clone()),
        Some(session_id.clone()),
    );
    video_dag.segment_index = Some(0);
    video_dag.segment_source = Some("video".to_string());
    let video_dag_id = video_dag.id.clone();
    dag_repo.insert(video_dag);

    manager
        .handle_dag_completion(DagCompletionInfo {
            dag_id: video_dag_id,
            streamer_id: Some(streamer_id.clone()),
            session_id: Some(session_id.clone()),
            succeeded: true,
            leaf_outputs: vec!["/v.mp4".to_string()],
        })
        .await;
    assert_eq!(manager.pipeline_coordinator.active_pair_count_inline(), 1);

    let mut danmu_dag = DagExecutionDbModel::new(
        &dag_def,
        Some(streamer_id.clone()),
        Some(session_id.clone()),
    );
    danmu_dag.segment_index = Some(0);
    danmu_dag.segment_source = Some("danmu".to_string());
    let danmu_dag_id = danmu_dag.id.clone();
    dag_repo.insert(danmu_dag);

    manager
        .handle_dag_completion(DagCompletionInfo {
            dag_id: danmu_dag_id,
            streamer_id: Some(streamer_id),
            session_id: Some(session_id.clone()),
            succeeded: true,
            leaf_outputs: vec!["/d.ass".to_string()],
        })
        .await;
    assert_eq!(manager.pipeline_coordinator.active_pair_count_inline(), 0);
}

fn test_session(id: &str, streamer_id: &str, end_time: Option<i64>) -> LiveSessionDbModel {
    LiveSessionDbModel {
        id: id.to_string(),
        streamer_id: Some(streamer_id.to_string()),
        streamer_name: Some(format!("Name of {streamer_id}")),
        start_time: chrono::Utc::now().timestamp_millis(),
        end_time,
        titles: Some("[]".to_string()),
        total_size_bytes: 1024,
    }
}

fn test_segment(session_id: &str, index: u32, path: &str) -> SessionSegmentDbModel {
    SessionSegmentDbModel::new(
        session_id,
        index,
        path,
        1.0,
        1024,
        SessionSegmentLifecycle::default(),
        SessionSegmentSplitReason::default(),
    )
}

fn test_coordination_dag(
    session_id: &str,
    streamer_id: &str,
    status: DagExecutionStatus,
    source: &str,
    segment_index: u32,
) -> DagExecutionDbModel {
    let dag_def = DagPipelineDefinition::new(
        "segment",
        vec![DagStep::new("A", PipelineStep::preset("remux"))],
    );
    let mut dag = DagExecutionDbModel::new(
        &dag_def,
        Some(streamer_id.to_string()),
        Some(session_id.to_string()),
    );
    dag.status = status.as_str().to_string();
    dag.segment_index = Some(i64::from(segment_index));
    dag.segment_source = Some(source.to_string());
    dag
}

async fn configured_recovery_services(
    streamer_id: &str,
    segment_pipeline: Option<&DagPipelineDefinition>,
    session_complete_pipeline: &DagPipelineDefinition,
) -> (
    Arc<SqlxConfigRepository>,
    Arc<SqlxStreamerRepository>,
    Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>,
) {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    sqlx::query("INSERT INTO platform_config (id, platform_name) VALUES (?, ?)")
        .bind("test-platform")
        .bind("test")
        .execute(&pool)
        .await
        .unwrap();

    let config_repo = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
    let mut global = config_repo.get_global_config().await.unwrap();
    global.pipeline = segment_pipeline.map(|pipeline| serde_json::to_string(pipeline).unwrap());
    global.session_complete_pipeline =
        Some(serde_json::to_string(session_complete_pipeline).unwrap());
    config_repo.update_global_config(&global).await.unwrap();

    let streamer_repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool));
    let mut streamer = crate::database::models::StreamerDbModel::new(
        "streamer",
        "https://example.com/streamer",
        "test-platform",
    );
    streamer.id = streamer_id.to_string();
    streamer_repo.create_streamer(&streamer).await.unwrap();

    let config_service = Arc::new(ConfigService::new(
        config_repo.clone(),
        streamer_repo.clone(),
    ));
    (config_repo, streamer_repo, config_service)
}

#[tokio::test]
async fn test_recovery_ignores_historical_ended_sessions() {
    let session_repo = Arc::new(TestSessionRepository::new(None));
    session_repo.insert_session(test_session(
        "old-session",
        "streamer-1",
        Some(chrono::Utc::now().timestamp_millis()),
    ));
    session_repo.insert_segment(test_segment("old-session", 0, "/missing-old.flv"));

    let dag_repo = Arc::new(TestDagRepository::new());
    let job_repo = Arc::new(TestJobRepository::new());
    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), job_repo)
            .with_session_repository(session_repo.clone())
            .with_dag_repository(dag_repo.clone());

    manager.recover_pipeline_coordination().await.unwrap();

    assert_eq!(
        manager.pipeline_coordinator.active_session_count_inline(),
        0,
        "startup recovery must not hydrate ordinary old ended sessions"
    );
    assert_eq!(
        dag_repo.create_calls(),
        0,
        "startup recovery must not replay historical segments into new DAGs"
    );
    assert_eq!(
        session_repo
            .list_filters()
            .last()
            .and_then(|filters| filters.active_only),
        Some(true),
        "recovery should query active sessions, not the full historical session list"
    );
}

#[tokio::test]
async fn recovery_recreates_missing_segment_dag_once_after_artifact_persistence() {
    let session_id = "missing-segment-dag";
    let streamer_id = "streamer-1";
    let segment_pipeline = DagPipelineDefinition::new(
        "segment",
        vec![DagStep::new(
            "segment-step",
            PipelineStep::inline("remux", serde_json::json!({})),
        )],
    );
    let session_pipeline = DagPipelineDefinition::new(
        "session",
        vec![DagStep::new(
            "session-step",
            PipelineStep::inline("remux", serde_json::json!({})),
        )],
    );
    let (_config_repo, streamer_repo, config_service) =
        configured_recovery_services(streamer_id, Some(&segment_pipeline), &session_pipeline).await;
    let session_repo = Arc::new(TestSessionRepository::new(None));
    session_repo.insert_session(test_session(
        session_id,
        streamer_id,
        Some(chrono::Utc::now().timestamp_millis()),
    ));
    session_repo.insert_segment(test_segment(session_id, 0, "/persisted.flv"));
    let dag_repo = Arc::new(TestDagRepository::new());
    let job_repo = Arc::new(TestJobRepository::new());
    let manager = PipelineManager::<SqlxConfigRepository, SqlxStreamerRepository>::with_repository(
        PipelineManagerConfig::default(),
        job_repo,
    )
    .with_session_repository(session_repo)
    .with_streamer_repository(streamer_repo)
    .with_config_service(config_service)
    .with_dag_repository(dag_repo.clone());

    manager.recover_pipeline_coordination().await.unwrap();
    manager.recover_pipeline_coordination().await.unwrap();

    assert_eq!(dag_repo.create_calls(), 1);
    let dags = dag_repo
        .list_dags(None, Some(session_id), 10, 0)
        .await
        .unwrap();
    assert_eq!(dags.len(), 1);
    assert_eq!(dags[0].segment_source.as_deref(), Some("video"));
    assert_eq!(dags[0].segment_index, Some(0));
}

#[tokio::test]
async fn recovery_recreates_missing_session_complete_dag_with_durable_marker() {
    let session_id = "missing-session-complete";
    let streamer_id = "streamer-1";
    let session_pipeline = DagPipelineDefinition::new(
        "session",
        vec![DagStep::new(
            "session-step",
            PipelineStep::inline("remux", serde_json::json!({})),
        )],
    );
    let (_config_repo, streamer_repo, config_service) =
        configured_recovery_services(streamer_id, None, &session_pipeline).await;
    let session_repo = Arc::new(TestSessionRepository::new(None));
    session_repo.insert_session(test_session(
        session_id,
        streamer_id,
        Some(chrono::Utc::now().timestamp_millis()),
    ));
    session_repo.insert_segment(test_segment(session_id, 0, "/persisted.flv"));
    let dag_repo = Arc::new(TestDagRepository::new());
    let job_repo = Arc::new(TestJobRepository::new());
    let manager = PipelineManager::<SqlxConfigRepository, SqlxStreamerRepository>::with_repository(
        PipelineManagerConfig::default(),
        job_repo,
    )
    .with_session_repository(session_repo)
    .with_streamer_repository(streamer_repo)
    .with_config_service(config_service)
    .with_dag_repository(dag_repo.clone());

    manager.recover_pipeline_coordination().await.unwrap();
    manager.recover_pipeline_coordination().await.unwrap();

    assert_eq!(dag_repo.create_calls(), 1);
    let dags = dag_repo
        .list_dags(None, Some(session_id), 10, 0)
        .await
        .unwrap();
    assert_eq!(dags.len(), 1);
    assert_eq!(dags[0].segment_source.as_deref(), Some("session_complete"));
    assert!(dags[0].segment_index.is_none());
}

/// An ended session that startup recovery reaches only through one of its
/// in-flight coordination DAGs (nothing else marks it as owing work) is
/// recovered as ended: the in-flight DAG is tracked rather than recreated,
/// and once it drains the session-complete workflow runs, exactly once.
#[tokio::test]
async fn recovery_finishes_an_ended_session_reached_only_through_an_in_flight_dag() {
    let session_id = "ended-in-flight";
    let streamer_id = "streamer-1";
    let session_pipeline = DagPipelineDefinition::new(
        "session",
        vec![DagStep::new(
            "session-step",
            PipelineStep::inline("remux", serde_json::json!({})),
        )],
    );
    let (_config_repo, streamer_repo, config_service) =
        configured_recovery_services(streamer_id, None, &session_pipeline).await;
    // No stored segment or output, so the session is not among the ended
    // sessions pending recovery; only its DAG pulls it in.
    let session_repo = Arc::new(TestSessionRepository::new(None));
    session_repo.insert_session(test_session(
        session_id,
        streamer_id,
        Some(chrono::Utc::now().timestamp_millis()),
    ));
    let dag_repo = Arc::new(TestDagRepository::new());
    let in_flight = test_coordination_dag(
        session_id,
        streamer_id,
        DagExecutionStatus::Processing,
        "video",
        0,
    );
    let in_flight_id = in_flight.id.clone();
    dag_repo.insert(in_flight);
    let manager = PipelineManager::<SqlxConfigRepository, SqlxStreamerRepository>::with_repository(
        PipelineManagerConfig::default(),
        Arc::new(TestJobRepository::new()),
    )
    .with_session_repository(session_repo)
    .with_streamer_repository(streamer_repo)
    .with_config_service(config_service)
    .with_dag_repository(dag_repo.clone());

    manager.recover_pipeline_coordination().await.unwrap();
    assert_eq!(
        dag_repo.create_calls(),
        0,
        "the in-flight DAG is tracked, not recreated"
    );

    manager
        .handle_dag_completion(DagCompletionInfo {
            dag_id: in_flight_id,
            streamer_id: Some(streamer_id.to_string()),
            session_id: Some(session_id.to_string()),
            succeeded: true,
            leaf_outputs: vec!["/processed.mp4".to_string()],
        })
        .await;

    assert_eq!(dag_repo.create_calls(), 1);
    let session_complete: Vec<_> = dag_repo
        .list_dags(None, Some(session_id), 10, 0)
        .await
        .unwrap()
        .into_iter()
        .filter(|dag| dag.segment_source.as_deref() == Some("session_complete"))
        .collect();
    assert_eq!(session_complete.len(), 1, "{session_complete:?}");
}

/// A session with nothing attached to it drains on the first observation, with no
/// wait at all. A session id that was never seen behaves the same, because
/// `drain_for_session` reads `job`, `dag_execution` and the coordinator only -
/// never `live_sessions`.
#[tokio::test]
async fn test_drain_for_session_returns_immediately_when_nothing_is_attached() {
    let dag_repo = Arc::new(TestDagRepository::new());
    let job_repo = Arc::new(TestJobRepository::new());
    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), job_repo.clone())
            .with_dag_repository(dag_repo.clone());

    let session_id = "drained-session";
    let mut done = JobDbModel::new_pipeline_step(
        "remux",
        "[]",
        "[]",
        0,
        Some("streamer-1".to_string()),
        Some(session_id.to_string()),
    );
    done.status = JobStatus::Completed.as_str().to_string();
    job_repo.insert(done);

    let mut finished_dag = DagExecutionDbModel::new(
        &DagPipelineDefinition::new(
            "segment",
            vec![DagStep::new(
                "A",
                PipelineStep::inline("remux", serde_json::json!({})),
            )],
        ),
        Some("streamer-1".to_string()),
        Some(session_id.to_string()),
    );
    finished_dag.status = DagExecutionStatus::Completed.as_str().to_string();
    dag_repo.insert(finished_dag);

    let started = std::time::Instant::now();
    assert_eq!(
        manager
            .drain_for_session(session_id, Duration::from_secs(30))
            .await
            .unwrap(),
        SessionDrain::Drained
    );
    assert_eq!(
        manager
            .drain_for_session("never-existed", Duration::ZERO)
            .await
            .unwrap(),
        SessionDrain::Drained
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "a quiescent session must not consume the bound"
    );
}

/// The drain returns only once the job it was waiting on leaves `PROCESSING`.
#[tokio::test]
async fn test_drain_for_session_waits_for_a_running_job() {
    let job_repo = Arc::new(TestJobRepository::new());
    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), job_repo.clone());

    let session_id = "running-job-session";
    let mut running = JobDbModel::new_pipeline_step(
        "remux",
        "[]",
        "[]",
        0,
        Some("streamer-1".to_string()),
        Some(session_id.to_string()),
    );
    running.status = JobStatus::Processing.as_str().to_string();
    let job_id = running.id.clone();
    job_repo.insert(running);

    assert_eq!(
        manager
            .drain_for_session(session_id, Duration::ZERO)
            .await
            .unwrap(),
        SessionDrain::Outstanding(SessionPipelineOutstanding {
            jobs: 1,
            ..Default::default()
        })
    );

    let finisher = {
        let job_repo = job_repo.clone();
        let job_id = job_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            job_repo.set_status(&job_id, JobStatus::Completed);
        })
    };

    let started = std::time::Instant::now();
    assert_eq!(
        manager
            .drain_for_session(session_id, Duration::from_secs(30))
            .await
            .unwrap(),
        SessionDrain::Drained
    );
    assert!(
        started.elapsed() >= Duration::from_millis(150),
        "the drain must not return before the job leaves PROCESSING"
    );
    finisher.await.unwrap();
}

/// When the caller's bound expires the drain names what it gave up on, rather
/// than reporting a bare timeout.
#[tokio::test]
async fn test_drain_for_session_reports_outstanding_work_at_the_bound() {
    let dag_repo = Arc::new(TestDagRepository::new());
    let job_repo = Arc::new(TestJobRepository::new());
    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), job_repo.clone())
            .with_dag_repository(dag_repo.clone());

    let session_id = "stuck-session";
    let mut running = JobDbModel::new_pipeline_step(
        "remux",
        "[]",
        "[]",
        0,
        Some("streamer-1".to_string()),
        Some(session_id.to_string()),
    );
    running.status = JobStatus::Processing.as_str().to_string();
    job_repo.insert(running);

    // A job on another session must not be counted against this one.
    let mut other = JobDbModel::new_pipeline_step(
        "remux",
        "[]",
        "[]",
        0,
        Some("streamer-2".to_string()),
        Some("other-session".to_string()),
    );
    other.status = JobStatus::Processing.as_str().to_string();
    job_repo.insert(other);

    let mut dag = DagExecutionDbModel::new(
        &DagPipelineDefinition::new(
            "segment",
            vec![DagStep::new(
                "A",
                PipelineStep::inline("remux", serde_json::json!({})),
            )],
        ),
        Some("streamer-1".to_string()),
        Some(session_id.to_string()),
    );
    dag.status = DagExecutionStatus::Processing.as_str().to_string();
    dag_repo.insert(dag);

    let started = std::time::Instant::now();
    let drain = manager
        .drain_for_session(session_id, Duration::from_millis(250))
        .await
        .unwrap();

    assert!(!drain.is_drained());
    assert_eq!(
        drain,
        SessionDrain::Outstanding(SessionPipelineOutstanding {
            jobs: 1,
            dags: 1,
            ..Default::default()
        })
    );
    assert!(
        started.elapsed() >= Duration::from_millis(250),
        "the drain must use the whole bound before reporting"
    );
}

/// `handle_dag_completion` reaches the coordinator after the `dag_execution` row is
/// already terminal, so a drain that only queried rows would call the session
/// quiescent while the coordinator is still about to create the session-complete
/// DAG.
#[tokio::test]
async fn test_drain_for_session_counts_coordinator_state_with_no_rows_left() {
    let dag_repo = Arc::new(TestDagRepository::new());
    let job_repo = Arc::new(TestJobRepository::new());
    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), job_repo.clone())
            .with_dag_repository(dag_repo.clone());

    let session_id = "coordinated-session".to_string();
    let streamer_id = "coordinated-streamer".to_string();
    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::SegmentDagStarted {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            segment_index: 0,
            source: SourceType::Video,
        });

    assert_eq!(
        manager
            .drain_for_session(&session_id, Duration::ZERO)
            .await
            .unwrap(),
        SessionDrain::Outstanding(SessionPipelineOutstanding {
            coordinated: crate::pipeline::SessionCoordinationOutstanding {
                end_processed: false,
                pending_dags: 1,
                session_complete_owed: false,
            },
            ..Default::default()
        })
    );

    manager.pipeline_coordinator.apply_event_inline(
        PipelineCoordinationEvent::SegmentDagCompleted {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            segment_index: 0,
            source: SourceType::Video,
            outputs: Vec::new(),
        },
    );

    assert_eq!(
        manager
            .drain_for_session(&session_id, Duration::ZERO)
            .await
            .unwrap(),
        SessionDrain::Drained
    );
}

/// `handle_session_transition` applies `ConfigureSession`, `SessionEnded` and
/// `SessionEndPersisted` as three separate reducer calls, and `try_finalize` only
/// emits the session-complete command on the last of them. A drain landing in
/// between sees no rows and no pending DAG counters, so it has to read
/// `session_complete_owed` to know the session-complete DAG is still coming.
#[tokio::test]
async fn test_drain_for_session_waits_for_an_owed_session_complete_dag() {
    let dag_repo = Arc::new(TestDagRepository::new());
    let job_repo = Arc::new(TestJobRepository::new());
    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), job_repo.clone())
            .with_dag_repository(dag_repo.clone());

    let session_id = "owed-session".to_string();
    let streamer_id = "owed-streamer".to_string();

    // Without a captured `session_complete_pipeline`, `try_finalize` settles the
    // session instead of emitting a command, and nothing is owed.
    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::ConfigureSession {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            danmu_enabled: false,
            segment_pipeline: None,
            paired_segment_pipeline: None,
            session_complete_pipeline: Some(DagPipelineDefinition::new(
                "session",
                vec![DagStep::new(
                    "session-step",
                    PipelineStep::inline("remux", serde_json::json!({})),
                )],
            )),
        });

    // One completed segment DAG, so `has_video_output` holds and the finalize
    // gate can open once the session end is persisted.
    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::SegmentDagStarted {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            segment_index: 0,
            source: SourceType::Video,
        });
    manager.pipeline_coordinator.apply_event_inline(
        PipelineCoordinationEvent::SegmentDagCompleted {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            segment_index: 0,
            source: SourceType::Video,
            outputs: vec![PathBuf::from("/out0.mp4")],
        },
    );

    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::SessionEnded {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            should_run_session_complete: true,
        });

    // `SessionEndPersisted` has not been applied, so `pending_session_complete_start`
    // is still false and every DAG counter is zero.
    let outstanding = manager.outstanding_for_session(&session_id).await.unwrap();
    assert_eq!(outstanding.jobs, 0);
    assert_eq!(outstanding.dags, 0);
    assert_eq!(outstanding.coordinated.pending_dags, 0);
    assert!(outstanding.coordinated.session_complete_owed);
    assert!(!outstanding.is_quiescent());

    assert!(matches!(
        manager
            .drain_for_session(&session_id, Duration::ZERO)
            .await
            .unwrap(),
        SessionDrain::Outstanding(_)
    ));

    // Still owed once the command has been emitted and before the DAG row exists.
    manager.pipeline_coordinator.apply_event_inline(
        PipelineCoordinationEvent::SessionEndPersisted {
            session_id: session_id.clone(),
        },
    );
    assert!(matches!(
        manager
            .drain_for_session(&session_id, Duration::ZERO)
            .await
            .unwrap(),
        SessionDrain::Outstanding(_)
    ));

    // `run_session_complete_pipeline` applies this only after
    // `create_dag_pipeline_internal` has published the DAG row, which is why the
    // coordinator is read before `dag_execution`.
    manager.pipeline_coordinator.apply_event_inline(
        PipelineCoordinationEvent::SessionCompleteDagStarted {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
        },
    );
    let mut dag = DagExecutionDbModel::new(
        &DagPipelineDefinition::new(
            "session",
            vec![DagStep::new(
                "A",
                PipelineStep::inline("remux", serde_json::json!({})),
            )],
        ),
        Some(streamer_id.clone()),
        Some(session_id.clone()),
    );
    dag.status = DagExecutionStatus::Processing.as_str().to_string();
    let dag_id = dag.id.clone();
    dag_repo.insert(dag);

    assert_eq!(
        manager
            .drain_for_session(&session_id, Duration::ZERO)
            .await
            .unwrap(),
        SessionDrain::Outstanding(SessionPipelineOutstanding {
            dags: 1,
            coordinated: crate::pipeline::SessionCoordinationOutstanding {
                end_processed: true,
                ..Default::default()
            },
            ..Default::default()
        })
    );

    dag_repo
        .dags
        .lock()
        .expect("lock poisoned")
        .get_mut(&dag_id)
        .expect("dag should exist")
        .status = DagExecutionStatus::Completed.as_str().to_string();

    assert_eq!(
        manager
            .drain_for_session(&session_id, Duration::ZERO)
            .await
            .unwrap(),
        SessionDrain::Drained
    );
}

/// A session that ends before any segment DAG completes can never satisfy the
/// finalize gate - `artifacts_drained_for_session_complete` requires
/// `has_video_output`, and nothing is left that could produce one. Reporting it
/// as owed would make every drain burn its whole bound on a session that
/// recorded nothing, which `TerminalCause::should_run_session_complete_pipeline`
/// makes routine: it is true for `Failed` as well as `StreamerOffline`.
#[tokio::test]
async fn test_drain_for_session_owes_nothing_when_the_finalize_gate_cannot_open() {
    let job_repo = Arc::new(TestJobRepository::new());
    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), job_repo);

    let session_id = "recorded-nothing".to_string();
    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::SessionEnded {
            session_id: session_id.clone(),
            streamer_id: "streamer-1".to_string(),
            should_run_session_complete: true,
        });
    manager.pipeline_coordinator.apply_event_inline(
        PipelineCoordinationEvent::SessionEndPersisted {
            session_id: session_id.clone(),
        },
    );

    let started = std::time::Instant::now();
    assert_eq!(
        manager
            .drain_for_session(&session_id, Duration::from_secs(30))
            .await
            .unwrap(),
        SessionDrain::Drained
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "a session whose finalize gate cannot open must not consume the bound"
    );
}

/// A creation failure leaves no durable DAG, so its dispatch remains owed until
/// recovery can publish it or a caller explicitly changes the pipeline policy.
#[tokio::test]
async fn test_drain_for_session_keeps_owing_after_a_failed_session_complete_creation() {
    let job_repo = Arc::new(TestJobRepository::new());
    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), job_repo);

    let session_id = "creation-failed".to_string();
    let streamer_id = "creation-failed-streamer".to_string();
    for event in [
        PipelineCoordinationEvent::SegmentDagStarted {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            segment_index: 0,
            source: SourceType::Video,
        },
        PipelineCoordinationEvent::SegmentDagCompleted {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            segment_index: 0,
            source: SourceType::Video,
            outputs: vec![PathBuf::from("/out0.mp4")],
        },
        PipelineCoordinationEvent::SessionEnded {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            should_run_session_complete: true,
        },
    ] {
        manager.pipeline_coordinator.apply_event_inline(event);
    }

    assert!(matches!(
        manager
            .drain_for_session(&session_id, Duration::ZERO)
            .await
            .unwrap(),
        SessionDrain::Outstanding(_)
    ));

    manager.pipeline_coordinator.apply_event_inline(
        PipelineCoordinationEvent::SessionCompleteDagCreationFailed {
            session_id: session_id.clone(),
        },
    );

    assert!(matches!(
        manager
            .drain_for_session(&session_id, Duration::ZERO)
            .await
            .unwrap(),
        SessionDrain::Outstanding(_)
    ));
}

/// `on_source_artifact` emits `CreateSegmentDag` before anything else records the
/// DAG, and `run_segment_pipeline` publishes its rows only after applying
/// `SegmentDagStarted`. `pending_segment_starts` is what covers that window, and
/// it is why `outstanding_for_session` can read the coordinator before the rows.
#[tokio::test]
async fn test_drain_for_session_counts_a_commanded_segment_dag_before_it_starts() {
    let dag_repo = Arc::new(TestDagRepository::new());
    let job_repo = Arc::new(TestJobRepository::new());
    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), job_repo.clone())
            .with_dag_repository(dag_repo.clone());

    let session_id = "commanded-segment".to_string();
    let streamer_id = "commanded-streamer".to_string();
    let segment_pipeline = DagPipelineDefinition::new(
        "segment",
        vec![DagStep::new(
            "A",
            PipelineStep::inline("remux", serde_json::json!({})),
        )],
    );

    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::ConfigureSession {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            danmu_enabled: false,
            segment_pipeline: Some(segment_pipeline),
            paired_segment_pipeline: None,
            session_complete_pipeline: None,
        });

    let commands = manager.pipeline_coordinator.apply_event_inline(
        PipelineCoordinationEvent::VideoSegmentCompleted {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            segment_index: 0,
            path: PathBuf::from("/segment0.flv"),
        },
    );
    assert!(matches!(
        commands.as_slice(),
        [PipelineCommand::CreateSegmentDag { .. }]
    ));

    // The command has been emitted and nothing has been published for it.
    assert_eq!(
        manager
            .drain_for_session(&session_id, Duration::ZERO)
            .await
            .unwrap(),
        SessionDrain::Outstanding(SessionPipelineOutstanding {
            coordinated: crate::pipeline::SessionCoordinationOutstanding {
                end_processed: false,
                pending_dags: 1,
                session_complete_owed: false,
            },
            ..Default::default()
        })
    );

    // `SegmentDagStarted` moves it from commanded to running, still outstanding.
    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::SegmentDagStarted {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            segment_index: 0,
            source: SourceType::Video,
        });
    assert_eq!(
        manager
            .outstanding_for_session(&session_id)
            .await
            .unwrap()
            .coordinated
            .pending_dags,
        1
    );

    manager.pipeline_coordinator.apply_event_inline(
        PipelineCoordinationEvent::SegmentDagCompleted {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            segment_index: 0,
            source: SourceType::Video,
            outputs: vec![PathBuf::from("/out0.mp4")],
        },
    );
    assert_eq!(
        manager
            .drain_for_session(&session_id, Duration::ZERO)
            .await
            .unwrap(),
        SessionDrain::Drained
    );
}
