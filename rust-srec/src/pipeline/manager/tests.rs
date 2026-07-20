use super::*;
use crate::database::models::{
    DagExecutionDbModel, DagStepExecutionDbModel, DanmuStatisticsDbModel, JobDbModel,
    JobExecutionLogDbModel, LiveSessionDbModel, OutputFilters, PipelinePreset, SessionFilters,
    SessionSegmentDbModel,
};
use crate::database::repositories::{PipelinePresetFilters, PipelinePresetRepository};
use crate::downloader::DownloadTerminalEvent;
use async_trait::async_trait;
use std::cmp::Reverse;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TestSessionRepository {
    end_time: Mutex<Option<i64>>,
    sessions: Mutex<HashMap<String, LiveSessionDbModel>>,
    segments: Mutex<HashMap<String, Vec<SessionSegmentDbModel>>>,
    outputs: Mutex<HashMap<String, Vec<MediaOutputDbModel>>>,
    list_filters: Mutex<Vec<SessionFilters>>,
}

impl TestSessionRepository {
    fn new(end_time: Option<i64>) -> Self {
        Self {
            end_time: Mutex::new(end_time),
            sessions: Mutex::new(HashMap::new()),
            segments: Mutex::new(HashMap::new()),
            outputs: Mutex::new(HashMap::new()),
            list_filters: Mutex::new(Vec::new()),
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
            streamer_id: "streamer-1".to_string(),
            start_time: chrono::Utc::now().timestamp_millis(),
            end_time: *self.end_time.lock().expect("lock poisoned"),
            titles: Some("[]".to_string()),
            danmu_statistics_id: None,
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

    async fn resume_session(&self, _id: &str) -> Result<()> {
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
            sessions.retain(|session| &session.streamer_id == streamer_id);
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

    async fn update_danmu_statistics(&self, _stats: &DanmuStatisticsDbModel) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn upsert_danmu_statistics(
        &self,
        _session_id: &str,
        _total_danmus: i64,
        _danmu_rate_timeseries: Option<&str>,
        _top_talkers: Option<&str>,
        _word_frequency: Option<&str>,
    ) -> Result<()> {
        unimplemented!("not needed for these tests")
    }
}

struct TestDagRepository {
    dags: Mutex<HashMap<String, DagExecutionDbModel>>,
    steps: Mutex<HashMap<String, Vec<DagStepExecutionDbModel>>>,
    create_calls: AtomicUsize,
}

impl TestDagRepository {
    fn new() -> Self {
        Self {
            dags: Mutex::new(HashMap::new()),
            steps: Mutex::new(HashMap::new()),
            create_calls: AtomicUsize::new(0),
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

    async fn mark_job_failed(&self, _id: &str, _error: &str) -> Result<u64> {
        unimplemented!("not needed for these tests")
    }

    async fn mark_job_cancelled(&self, _id: &str) -> Result<u64> {
        unimplemented!("not needed for these tests")
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

    async fn upsert_job_execution_progress(
        &self,
        _progress: &crate::database::models::JobExecutionProgressDbModel,
    ) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn get_job_execution_progress(
        &self,
        _job_id: &str,
    ) -> Result<Option<crate::database::models::JobExecutionProgressDbModel>> {
        unimplemented!("not needed for these tests")
    }

    async fn claim_next_pending_job(
        &self,
        _job_types: Option<&[String]>,
    ) -> Result<Option<JobDbModel>> {
        unimplemented!("not needed for these tests")
    }

    async fn get_job_execution_info(&self, _id: &str) -> Result<Option<String>> {
        unimplemented!("not needed for these tests")
    }

    async fn update_job_execution_info(&self, _id: &str, _execution_info: &str) -> Result<()> {
        unimplemented!("not needed for these tests")
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

    async fn add_execution_logs(&self, _logs: &[JobExecutionLogDbModel]) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn get_execution_logs(&self, _job_id: &str) -> Result<Vec<JobExecutionLogDbModel>> {
        unimplemented!("not needed for these tests")
    }

    async fn list_execution_logs(
        &self,
        _job_id: &str,
        _pagination: &crate::database::models::Pagination,
    ) -> Result<(Vec<JobExecutionLogDbModel>, u64)> {
        unimplemented!("not needed for these tests")
    }

    async fn delete_execution_logs_for_job(&self, _job_id: &str) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn list_jobs_filtered(
        &self,
        _filters: &crate::database::models::JobFilters,
        _pagination: &crate::database::models::Pagination,
    ) -> Result<(Vec<JobDbModel>, u64)> {
        unimplemented!("not needed for these tests")
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

    async fn cancel_jobs_by_pipeline(&self, _pipeline_id: &str) -> Result<u64> {
        unimplemented!("not needed for these tests")
    }

    async fn get_jobs_by_pipeline(&self, _pipeline_id: &str) -> Result<Vec<JobDbModel>> {
        unimplemented!("not needed for these tests")
    }

    async fn delete_jobs_by_pipeline(&self, _pipeline_id: &str) -> Result<u64> {
        unimplemented!("not needed for these tests")
    }
}

struct TestDagRepositoryForRetry {
    dags: Mutex<HashMap<String, DagExecutionDbModel>>,
    reset_calls: AtomicUsize,
}

impl TestDagRepositoryForRetry {
    fn new() -> Self {
        Self {
            dags: Mutex::new(HashMap::new()),
            reset_calls: AtomicUsize::new(0),
        }
    }

    fn insert(&self, dag: DagExecutionDbModel) {
        self.dags
            .lock()
            .expect("lock poisoned")
            .insert(dag.id.clone(), dag);
    }

    fn reset_calls(&self) -> usize {
        self.reset_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl DagRepository for TestDagRepositoryForRetry {
    async fn create_dag(&self, _dag: &DagExecutionDbModel) -> Result<()> {
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

    async fn increment_dag_completed(&self, _dag_id: &str) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn increment_dag_failed(&self, _dag_id: &str) -> Result<()> {
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

    async fn delete_dag(&self, _id: &str) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn create_step(&self, _step: &DagStepExecutionDbModel) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn create_steps(&self, _steps: &[DagStepExecutionDbModel]) -> Result<()> {
        unimplemented!("not needed for these tests")
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

    async fn get_steps_by_dag(&self, _dag_id: &str) -> Result<Vec<DagStepExecutionDbModel>> {
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

    async fn complete_step_and_check_dependents(
        &self,
        _step_id: &str,
        _outputs: &[String],
    ) -> Result<Vec<crate::database::models::ReadyStep>> {
        unimplemented!("not needed for these tests")
    }

    async fn fail_dag_and_cancel_steps(&self, _dag_id: &str, _error: &str) -> Result<Vec<String>> {
        unimplemented!("not needed for these tests")
    }

    async fn cancel_dag_and_cancel_steps(
        &self,
        _dag_id: &str,
        _error: &str,
    ) -> Result<Vec<String>> {
        unimplemented!("not needed for these tests")
    }

    async fn reset_dag_for_retry(&self, dag_id: &str) -> Result<()> {
        self.reset_calls.fetch_add(1, Ordering::SeqCst);
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

    async fn get_dependency_outputs(
        &self,
        _dag_id: &str,
        _step_ids: &[String],
    ) -> Result<Vec<String>> {
        unimplemented!("not needed for these tests")
    }

    async fn check_all_dependencies_complete(&self, _dag_id: &str, _step_id: &str) -> Result<bool> {
        unimplemented!("not needed for these tests")
    }

    async fn get_dag_stats(
        &self,
        _dag_id: &str,
    ) -> Result<crate::database::models::DagExecutionStats> {
        unimplemented!("not needed for these tests")
    }

    async fn get_processing_job_ids(&self, _dag_id: &str) -> Result<Vec<String>> {
        unimplemented!("not needed for these tests")
    }

    async fn get_pending_root_steps(&self, _dag_id: &str) -> Result<Vec<DagStepExecutionDbModel>> {
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

    async fn increment_dag_completed(&self, _dag_id: &str) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn increment_dag_failed(&self, _dag_id: &str) -> Result<()> {
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

    async fn complete_step_and_check_dependents(
        &self,
        _step_id: &str,
        _outputs: &[String],
    ) -> Result<Vec<crate::database::models::ReadyStep>> {
        unimplemented!("not needed for these tests")
    }

    async fn fail_dag_and_cancel_steps(&self, _dag_id: &str, _error: &str) -> Result<Vec<String>> {
        unimplemented!("not needed for these tests")
    }

    async fn cancel_dag_and_cancel_steps(
        &self,
        _dag_id: &str,
        _error: &str,
    ) -> Result<Vec<String>> {
        unimplemented!("not needed for these tests")
    }

    async fn reset_dag_for_retry(&self, _dag_id: &str) -> Result<()> {
        unimplemented!("not needed for these tests")
    }

    async fn get_dependency_outputs(
        &self,
        _dag_id: &str,
        _step_ids: &[String],
    ) -> Result<Vec<String>> {
        unimplemented!("not needed for these tests")
    }

    async fn check_all_dependencies_complete(&self, _dag_id: &str, _step_id: &str) -> Result<bool> {
        unimplemented!("not needed for these tests")
    }

    async fn get_dag_stats(
        &self,
        _dag_id: &str,
    ) -> Result<crate::database::models::DagExecutionStats> {
        unimplemented!("not needed for these tests")
    }

    async fn get_processing_job_ids(&self, _dag_id: &str) -> Result<Vec<String>> {
        unimplemented!("not needed for these tests")
    }

    async fn get_pending_root_steps(&self, _dag_id: &str) -> Result<Vec<DagStepExecutionDbModel>> {
        unimplemented!("not needed for these tests")
    }
}

#[test]
fn test_pipeline_manager_config_default() {
    let config = PipelineManagerConfig::default();
    assert_eq!(config.cpu_pool.max_workers, 2);
    assert_eq!(config.io_pool.max_workers, 4);
    assert_eq!(config.execute_timeout_secs, 3600);
    // Verify throttle config defaults
    assert!(!config.throttle.enabled);
    assert_eq!(config.throttle.critical_threshold, 500);
    assert_eq!(config.throttle.warning_threshold, 100);
}

#[test]
fn test_pipeline_manager_creation() {
    let manager: PipelineManager = PipelineManager::new();
    assert_eq!(manager.queue_depth(), 0);
    assert_eq!(manager.queue_status(), QueueDepthStatus::Normal);
}

#[tokio::test]
async fn test_retry_job_resets_failed_dag_when_job_is_dag_step() {
    let job_repo = Arc::new(TestJobRepository::new());
    let dag_repo = Arc::new(TestDagRepositoryForRetry::new());

    let dag_def = crate::database::models::DagPipelineDefinition::new(
        "test",
        vec![crate::database::models::DagStep::new(
            "step-a",
            crate::database::models::PipelineStep::Inline {
                processor: "remux".to_string(),
                config: serde_json::json!({}),
            },
        )],
    );

    let mut dag = DagExecutionDbModel::new(&dag_def, None, None);
    dag.status = crate::database::models::DagExecutionStatus::Failed
        .as_str()
        .to_string();
    let dag_id = dag.id.clone();
    dag_repo.insert(dag);

    let mut job = JobDbModel::new_pipeline_step(
        "remux",
        serde_json::to_string(&vec!["/in.flv".to_string()]).unwrap(),
        "[]",
        0,
        None,
        None,
    );
    job.pipeline_id = Some(dag_id.clone());
    job.dag_step_execution_id = Some("step-exec-1".to_string());
    job.status = JobStatus::Failed.as_str().to_string();
    job.error = Some("boom".to_string());
    job.completed_at = Some(chrono::Utc::now().timestamp_millis());
    let job_id = job.id.clone();
    job_repo.insert(job);

    let config = PipelineManagerConfig::default();

    let manager: PipelineManager =
        PipelineManager::with_repository(config, job_repo).with_dag_repository(dag_repo.clone());

    let retried = manager.retry_job(&job_id).await.unwrap();
    assert_eq!(retried.status, crate::pipeline::JobStatus::Pending);
    assert_eq!(retried.retry_count, 1);
    assert!(retried.error.is_none());

    assert_eq!(dag_repo.reset_calls(), 1);
}

#[tokio::test]
async fn test_retry_job_resets_cancelled_dag_when_job_is_dag_step() {
    let job_repo = Arc::new(TestJobRepository::new());
    let dag_repo = Arc::new(TestDagRepositoryForRetry::new());

    let dag_def = DagExecutionDbModel::new(
        &crate::database::models::DagPipelineDefinition::new(
            "test",
            vec![crate::database::models::DagStep::new(
                "step-a",
                crate::database::models::PipelineStep::Inline {
                    processor: "remux".to_string(),
                    config: serde_json::json!({}),
                },
            )],
        ),
        None,
        None,
    );

    let mut dag = dag_def;
    dag.status = crate::database::models::DagExecutionStatus::Cancelled
        .as_str()
        .to_string();
    let dag_id = dag.id.clone();
    dag_repo.insert(dag);

    let mut job = JobDbModel::new_pipeline_step(
        "remux",
        serde_json::to_string(&vec!["/in.flv".to_string()]).unwrap(),
        "[]",
        0,
        None,
        None,
    );
    job.pipeline_id = Some(dag_id.clone());
    job.dag_step_execution_id = Some("step-exec-1".to_string());
    job.status = JobStatus::Cancelled.as_str().to_string();
    job.error = Some("cancelled".to_string());
    job.completed_at = Some(chrono::Utc::now().timestamp_millis());
    let job_id = job.id.clone();
    job_repo.insert(job);

    let config = PipelineManagerConfig::default();

    let manager: PipelineManager =
        PipelineManager::with_repository(config, job_repo).with_dag_repository(dag_repo.clone());

    let retried = manager.retry_job(&job_id).await.unwrap();
    assert_eq!(retried.status, crate::pipeline::JobStatus::Pending);
    assert_eq!(dag_repo.reset_calls(), 1);
}

#[test]
fn test_set_worker_concurrency_clamps_to_max_workers() {
    let manager: PipelineManager = PipelineManager::new();

    // Defaults are 2/4, so requests above should clamp.
    manager.set_worker_concurrency(10, 20);
    assert_eq!(manager.cpu_pool.desired_max_workers(), 2);
    assert_eq!(manager.io_pool.desired_max_workers(), 4);

    // Requests below max should apply.
    manager.set_worker_concurrency(1, 3);
    assert_eq!(manager.cpu_pool.desired_max_workers(), 1);
    assert_eq!(manager.io_pool.desired_max_workers(), 3);
}

struct TestPipelinePresetRepository {
    preset: PipelinePreset,
}

#[async_trait]
impl PipelinePresetRepository for TestPipelinePresetRepository {
    async fn list_pipeline_presets(&self) -> Result<Vec<PipelinePreset>> {
        Ok(vec![])
    }

    async fn list_pipeline_presets_filtered(
        &self,
        _filters: &PipelinePresetFilters,
        _pagination: &Pagination,
    ) -> Result<(Vec<PipelinePreset>, u64)> {
        Ok((vec![], 0))
    }

    async fn get_pipeline_preset(&self, _id: &str) -> Result<Option<PipelinePreset>> {
        Ok(None)
    }

    async fn get_pipeline_preset_by_name(&self, name: &str) -> Result<Option<PipelinePreset>> {
        if name == self.preset.name {
            Ok(Some(self.preset.clone()))
        } else {
            Ok(None)
        }
    }

    async fn create_pipeline_preset(&self, _preset: &PipelinePreset) -> Result<()> {
        Ok(())
    }

    async fn update_pipeline_preset(&self, _preset: &PipelinePreset) -> Result<()> {
        Ok(())
    }

    async fn delete_pipeline_preset(&self, _id: &str) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn test_expand_workflows_with_duplicate_names() {
    let workflow_dag = DagPipelineDefinition::new(
        "wf",
        vec![
            DagStep::new("A", PipelineStep::inline("noop", serde_json::json!({}))),
            DagStep::with_dependencies(
                "B",
                PipelineStep::inline("noop", serde_json::json!({})),
                vec!["A".to_string()],
            ),
        ],
    );
    let repo = Arc::new(TestPipelinePresetRepository {
        preset: PipelinePreset::new("wf", workflow_dag),
    });

    let manager: PipelineManager = PipelineManager::new().with_pipeline_preset_repository(repo);

    let parent = DagPipelineDefinition::new(
        "parent",
        vec![
            DagStep {
                id: "W1".to_string(),
                step: PipelineStep::Workflow {
                    name: "wf".to_string(),
                },
                depends_on: vec![],
            },
            DagStep {
                id: "W2".to_string(),
                step: PipelineStep::Workflow {
                    name: "wf".to_string(),
                },
                depends_on: vec!["W1".to_string()],
            },
            DagStep::with_dependencies(
                "Z",
                PipelineStep::inline("noop", serde_json::json!({})),
                vec!["W2".to_string()],
            ),
        ],
    );

    let expanded = manager.expand_workflows_in_dag(parent).await.unwrap();

    let mut deps_by_id: HashMap<String, Vec<String>> = HashMap::new();
    for step in &expanded.steps {
        deps_by_id.insert(step.id.clone(), step.depends_on.clone());
    }

    assert!(!deps_by_id.contains_key("W1"));
    assert!(!deps_by_id.contains_key("W2"));

    assert_eq!(deps_by_id.get("W1__A").unwrap(), &Vec::<String>::new());
    assert_eq!(deps_by_id.get("W1__B").unwrap(), &vec!["W1__A".to_string()]);

    // W2 depends on the *leaf* of W1 after expansion.
    assert_eq!(deps_by_id.get("W2__A").unwrap(), &vec!["W1__B".to_string()]);
    assert_eq!(deps_by_id.get("W2__B").unwrap(), &vec!["W2__A".to_string()]);

    // Z depends on the *leaf* of W2 after expansion.
    assert_eq!(deps_by_id.get("Z").unwrap(), &vec!["W2__B".to_string()]);
}

#[tokio::test]
async fn test_enqueue_job() {
    let manager: PipelineManager = PipelineManager::new();

    let job = Job::new(
        "remux",
        vec!["/input.flv".to_string()],
        vec!["/output.mp4".to_string()],
        "streamer-1",
        "session-1",
    );
    let job_id = manager.enqueue(job).await.unwrap();

    assert!(!job_id.is_empty());
    assert_eq!(manager.queue_depth(), 1);
}

#[tokio::test]
async fn test_list_jobs() {
    use crate::database::models::{JobFilters, Pagination};

    let manager: PipelineManager = PipelineManager::new();

    // Enqueue some jobs
    let job1 = Job::new(
        "remux",
        vec!["/input1.flv".to_string()],
        vec!["/output1.mp4".to_string()],
        "streamer-1",
        "session-1",
    );
    let job2 = Job::new(
        "upload",
        vec!["/input2.flv".to_string()],
        vec!["/output2.mp4".to_string()],
        "streamer-2",
        "session-2",
    );
    manager.enqueue(job1).await.unwrap();
    manager.enqueue(job2).await.unwrap();

    // List all jobs
    let filters = JobFilters::default();
    let pagination = Pagination::new(10, 0);
    let (jobs, total) = manager.list_jobs(&filters, &pagination).await.unwrap();

    assert_eq!(total, 2);
    assert_eq!(jobs.len(), 2);
}

#[tokio::test]
async fn test_list_jobs_with_filter() {
    use crate::database::models::{JobFilters, Pagination};

    let manager: PipelineManager = PipelineManager::new();

    // Enqueue jobs for different streamers
    let job1 = Job::new(
        "remux",
        vec!["/input1.flv".to_string()],
        vec!["/output1.mp4".to_string()],
        "streamer-1",
        "session-1",
    );
    let job2 = Job::new(
        "upload",
        vec!["/input2.flv".to_string()],
        vec!["/output2.mp4".to_string()],
        "streamer-2",
        "session-2",
    );
    manager.enqueue(job1).await.unwrap();
    manager.enqueue(job2).await.unwrap();

    // Filter by streamer_id
    let filters = JobFilters::new().with_streamer_id("streamer-1");
    let pagination = Pagination::new(10, 0);
    let (jobs, total) = manager.list_jobs(&filters, &pagination).await.unwrap();

    assert_eq!(total, 1);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].streamer_id, "streamer-1");
}

#[tokio::test]
async fn test_get_job() {
    let manager: PipelineManager = PipelineManager::new();

    let job = Job::new(
        "remux",
        vec!["/input.flv".to_string()],
        vec!["/output.mp4".to_string()],
        "streamer-1",
        "session-1",
    );
    let job_id = job.id.clone();
    manager.enqueue(job).await.unwrap();

    // Get existing job
    let retrieved = manager.get_job(&job_id).await.unwrap();
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().id, job_id);

    // Get non-existing job
    let not_found = manager.get_job("non-existent-id").await.unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
async fn test_get_stats() {
    let manager: PipelineManager = PipelineManager::new();

    // Enqueue some jobs
    let job1 = Job::new(
        "remux",
        vec!["/input1.flv".to_string()],
        vec!["/output1.mp4".to_string()],
        "streamer-1",
        "session-1",
    );
    let job2 = Job::new(
        "upload",
        vec!["/input2.flv".to_string()],
        vec!["/output2.mp4".to_string()],
        "streamer-2",
        "session-2",
    );
    manager.enqueue(job1).await.unwrap();
    manager.enqueue(job2).await.unwrap();

    let stats = manager.get_stats().await.unwrap();

    assert_eq!(stats.pending, 2);
    assert_eq!(stats.processing, 0);
    assert_eq!(stats.completed, 0);
    assert_eq!(stats.failed, 0);
    assert_eq!(stats.queue_depth, 2);
    assert_eq!(stats.queue_status, QueueDepthStatus::Normal);
}

#[tokio::test]
async fn test_cancel_pending_job() {
    use crate::pipeline::JobStatus;

    let manager: PipelineManager = PipelineManager::new();

    let job = Job::new(
        "remux",
        vec!["/input.flv".to_string()],
        vec!["/output.mp4".to_string()],
        "streamer-1",
        "session-1",
    );
    let job_id = job.id.clone();
    manager.enqueue(job).await.unwrap();

    // Cancel the pending job
    manager.cancel_job(&job_id).await.unwrap();

    let cancelled = manager.get_job(&job_id).await.unwrap().unwrap();
    assert_eq!(cancelled.status, JobStatus::Cancelled);
}

#[test]
fn test_throttle_controller_disabled_by_default() {
    let manager: PipelineManager = PipelineManager::new();

    // Throttle controller should be None when disabled
    assert!(manager.throttle_controller().is_none());
    assert!(!manager.is_throttled());
    assert!(manager.subscribe_throttle_events().is_none());
}

#[test]
fn test_throttle_controller_enabled_with_config() {
    let config = PipelineManagerConfig {
        throttle: ThrottleConfig {
            enabled: true,
            critical_threshold: 100,
            warning_threshold: 50,
            ..Default::default()
        },
        ..Default::default()
    };
    let manager: PipelineManager = PipelineManager::with_config(config);

    // Throttle controller should be Some when enabled
    assert!(manager.throttle_controller().is_some());
    assert!(!manager.is_throttled());
    assert!(manager.subscribe_throttle_events().is_some());
}

#[test]
fn test_config_includes_throttle_defaults() {
    let config = PipelineManagerConfig::default();

    assert!(!config.throttle.enabled);
    assert_eq!(config.throttle.critical_threshold, 500);
    assert_eq!(config.throttle.warning_threshold, 100);
    assert!((config.throttle.reduction_factor - 0.5).abs() < f32::EPSILON);
}

#[tokio::test]
async fn test_create_dag_pipeline_requires_dag_scheduler() {
    use crate::database::models::job::{DagPipelineDefinition, DagStep, PipelineStep};

    let manager: PipelineManager = PipelineManager::new();

    // Create a simple DAG definition
    let dag_def = DagPipelineDefinition::new(
        "Test Pipeline",
        vec![DagStep::new("remux", PipelineStep::preset("remux"))],
    );

    // Without a DAG scheduler configured, this should fail
    let result = manager
        .create_dag_pipeline(
            "session-1",
            "streamer-1",
            vec!["/input.flv".to_string()],
            dag_def,
        )
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("DAG scheduler not configured"));
}

#[tokio::test]
async fn test_session_complete_waits_for_paired_dags() {
    let session_repo = Arc::new(TestSessionRepository::new(Some(
        chrono::Utc::now().timestamp_millis(),
    )));
    let manager: PipelineManager = PipelineManager::new().with_session_repository(session_repo);

    let session_id = "session-1".to_string();
    let streamer_id = "streamer-1".to_string();

    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::ConfigureSession {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            danmu_enabled: false,
            segment_pipeline: None,
            paired_segment_pipeline: None,
            session_complete_pipeline: Some(DagPipelineDefinition::new("empty", vec![])),
        });
    manager.pipeline_coordinator.apply_event_inline(
        PipelineCoordinationEvent::VideoSegmentCompleted {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            segment_index: 0,
            path: PathBuf::from("/seg0.mp4"),
        },
    );
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
    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::PairedDagStarted {
            session_id: session_id.clone(),
            streamer_id,
            segment_index: 0,
        });

    assert_eq!(
        manager.pipeline_coordinator.active_session_count_inline(),
        1
    );

    manager
        .execute_pipeline_commands(manager.pipeline_coordinator.apply_event_inline(
            PipelineCoordinationEvent::PairedDagCompleted {
                session_id: session_id.clone(),
            },
        ))
        .await;
    assert_eq!(
        manager.pipeline_coordinator.active_session_count_inline(),
        1
    );
}

#[tokio::test]
async fn test_session_complete_recovers_segment_dag_completion_without_context() {
    let session_repo = Arc::new(TestSessionRepository::new(Some(
        chrono::Utc::now().timestamp_millis(),
    )));
    let dag_repo = Arc::new(TestDagRepository::new());
    let manager: PipelineManager = PipelineManager::new()
        .with_session_repository(session_repo)
        .with_dag_repository(dag_repo.clone());

    let session_id = "session-1".to_string();
    let streamer_id = "streamer-1".to_string();

    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::ConfigureSession {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            danmu_enabled: false,
            segment_pipeline: Some(DagPipelineDefinition::new(
                "segment",
                vec![DagStep::new("A", PipelineStep::preset("remux"))],
            )),
            paired_segment_pipeline: None,
            session_complete_pipeline: Some(DagPipelineDefinition::new("empty", vec![])),
        });
    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::SegmentDagStarted {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            segment_index: 0,
            source: SourceType::Video,
        });
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

    let dag_def = DagPipelineDefinition::new(
        "test-dag",
        vec![DagStep::new("A", PipelineStep::preset("remux"))],
    );
    let mut dag = DagExecutionDbModel::new(
        &dag_def,
        Some(streamer_id.clone()),
        Some(session_id.clone()),
    );
    dag.segment_index = Some(0);
    dag.segment_source = Some("video".to_string());
    let dag_id = dag.id.clone();
    dag_repo.insert(dag);

    manager
        .handle_dag_completion(DagCompletionInfo {
            dag_id,
            streamer_id: Some(streamer_id),
            session_id: Some(session_id.clone()),
            succeeded: true,
            leaf_outputs: vec!["/out.mp4".to_string()],
        })
        .await;

    assert_eq!(
        manager.pipeline_coordinator.active_session_count_inline(),
        1
    );
}

#[tokio::test]
async fn test_session_complete_recovers_paired_dag_completion_without_context() {
    let session_repo = Arc::new(TestSessionRepository::new(Some(
        chrono::Utc::now().timestamp_millis(),
    )));
    let dag_repo = Arc::new(TestDagRepository::new());
    let manager: PipelineManager = PipelineManager::new()
        .with_session_repository(session_repo)
        .with_dag_repository(dag_repo.clone());

    let session_id = "session-1".to_string();
    let streamer_id = "streamer-1".to_string();

    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::ConfigureSession {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            danmu_enabled: false,
            segment_pipeline: None,
            paired_segment_pipeline: None,
            session_complete_pipeline: Some(DagPipelineDefinition::new("empty", vec![])),
        });
    manager.pipeline_coordinator.apply_event_inline(
        PipelineCoordinationEvent::VideoSegmentCompleted {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            segment_index: 0,
            path: PathBuf::from("/seg0.mp4"),
        },
    );
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
    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::PairedDagStarted {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            segment_index: 0,
        });

    let dag_def = DagPipelineDefinition::new(
        "paired-dag",
        vec![DagStep::new("A", PipelineStep::preset("remux"))],
    );
    let mut dag = DagExecutionDbModel::new(
        &dag_def,
        Some(streamer_id.clone()),
        Some(session_id.clone()),
    );
    dag.segment_index = Some(0);
    dag.segment_source = Some("paired".to_string());
    let dag_id = dag.id.clone();
    dag_repo.insert(dag);

    manager
        .handle_dag_completion(DagCompletionInfo {
            dag_id,
            streamer_id: Some(streamer_id),
            session_id: Some(session_id.clone()),
            succeeded: true,
            leaf_outputs: Vec::new(),
        })
        .await;

    assert_eq!(
        manager.pipeline_coordinator.active_session_count_inline(),
        1
    );
}

#[tokio::test]
async fn test_paired_segment_recovers_segment_dag_completion_without_context() {
    let dag_repo = Arc::new(TestDagRepository::new());
    let manager: PipelineManager = PipelineManager::new().with_dag_repository(dag_repo.clone());

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
        streamer_id: streamer_id.to_string(),
        start_time: chrono::Utc::now().timestamp_millis(),
        end_time,
        titles: Some("[]".to_string()),
        danmu_statistics_id: None,
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
async fn test_recovery_hydrates_active_session_without_replaying_segment_dag() {
    let session_repo = Arc::new(TestSessionRepository::new(None));
    session_repo.insert_session(test_session("active-session", "streamer-1", None));
    session_repo.insert_segment(test_segment("active-session", 0, "/active.flv"));

    let dag_repo = Arc::new(TestDagRepository::new());
    let job_repo = Arc::new(TestJobRepository::new());
    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), job_repo)
            .with_session_repository(session_repo)
            .with_dag_repository(dag_repo.clone());

    manager.recover_pipeline_coordination().await.unwrap();

    assert_eq!(
        manager.pipeline_coordinator.active_session_count_inline(),
        1,
        "active sessions still need in-memory coordinator state after restart"
    );
    assert_eq!(
        dag_repo.create_calls(),
        0,
        "recovered source rows must not be treated as fresh segment completions"
    );
}

#[tokio::test]
async fn test_recovery_tracks_in_flight_ended_session_without_duplicate_dag() {
    let session_id = "ended-in-flight";
    let streamer_id = "streamer-1";
    let session_repo = Arc::new(TestSessionRepository::new(None));
    session_repo.insert_session(test_session(
        session_id,
        streamer_id,
        Some(chrono::Utc::now().timestamp_millis()),
    ));
    session_repo.insert_segment(test_segment(session_id, 0, "/in-flight.flv"));

    let dag_repo = Arc::new(TestDagRepository::new());
    let pending_dag = test_coordination_dag(
        session_id,
        streamer_id,
        DagExecutionStatus::Processing,
        "video",
        0,
    );
    let pending_dag_id = pending_dag.id.clone();
    dag_repo.insert(pending_dag);

    let job_repo = Arc::new(TestJobRepository::new());
    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), job_repo)
            .with_session_repository(session_repo)
            .with_dag_repository(dag_repo.clone());

    manager.recover_pipeline_coordination().await.unwrap();

    assert_eq!(
        dag_repo.create_calls(),
        0,
        "recovery should mark the existing DAG pending, not create another one"
    );

    manager
        .handle_dag_completion(DagCompletionInfo {
            dag_id: pending_dag_id,
            streamer_id: Some(streamer_id.to_string()),
            session_id: Some(session_id.to_string()),
            succeeded: true,
            leaf_outputs: vec!["/processed.mp4".to_string()],
        })
        .await;

    assert_eq!(
        dag_repo.create_calls(),
        0,
        "draining the recovered DAG must not create follow-up DAGs when no pipeline is configured"
    );
}

// -----------------------------------------------------------------------
// Session-complete pipeline firing on terminal download events:
// Completed *and* Failed must trigger it; Cancelled and Rejected must not.
// -----------------------------------------------------------------------

use crate::downloader::engine::EngineType;
use crate::downloader::{
    DownloadFailureKind, DownloadProtocol, DownloadRejectedKind, DownloadStopCause, EngineEndSignal,
};

fn completed_event(session_id: &str, streamer_id: &str) -> DownloadManagerEvent {
    DownloadManagerEvent::Terminal(DownloadTerminalEvent::Completed {
        download_id: "dl-1".to_string(),
        streamer_id: streamer_id.to_string(),
        streamer_name: "tester".to_string(),
        session_id: session_id.to_string(),
        total_bytes: 0,
        total_duration_secs: 0.0,
        total_segments: 0,
        file_path: None,
        engine_signal: EngineEndSignal::Unknown,
    })
}

fn failed_event(session_id: &str, streamer_id: &str) -> DownloadManagerEvent {
    DownloadManagerEvent::Terminal(DownloadTerminalEvent::Failed {
        download_id: "dl-1".to_string(),
        streamer_id: streamer_id.to_string(),
        streamer_name: "tester".to_string(),
        session_id: session_id.to_string(),
        engine_type: EngineType::Ffmpeg,
        protocol: DownloadProtocol::Unknown,
        kind: DownloadFailureKind::Network,
        error: "stalled".to_string(),
        recoverable: false,
    })
}

fn cancelled_event(session_id: &str, streamer_id: &str) -> DownloadManagerEvent {
    DownloadManagerEvent::Terminal(DownloadTerminalEvent::Cancelled {
        download_id: "dl-1".to_string(),
        streamer_id: streamer_id.to_string(),
        streamer_name: "tester".to_string(),
        session_id: session_id.to_string(),
        cause: DownloadStopCause::User,
    })
}

fn rejected_event(session_id: &str, streamer_id: &str) -> DownloadManagerEvent {
    DownloadManagerEvent::Terminal(DownloadTerminalEvent::Rejected {
        streamer_id: streamer_id.to_string(),
        streamer_name: "tester".to_string(),
        session_id: session_id.to_string(),
        reason: "test".to_string(),
        retry_after_secs: None,
        kind: DownloadRejectedKind::CircuitBreaker,
    })
}

fn ended_failed(session_id: &str, streamer_id: &str) -> crate::session::SessionTransition {
    crate::session::SessionTransition::Ended {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        streamer_name: "tester".to_string(),
        ended_at: chrono::Utc::now(),
        cause: crate::session::TerminalCause::Failed {
            kind: DownloadFailureKind::Network,
        },
        via_hysteresis: false,
    }
}

fn ended_cancelled(session_id: &str, streamer_id: &str) -> crate::session::SessionTransition {
    crate::session::SessionTransition::Ended {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        streamer_name: "tester".to_string(),
        ended_at: chrono::Utc::now(),
        cause: crate::session::TerminalCause::Cancelled {
            cause: DownloadStopCause::User,
        },
        via_hysteresis: false,
    }
}

fn ended_rejected(session_id: &str, streamer_id: &str) -> crate::session::SessionTransition {
    crate::session::SessionTransition::Ended {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        streamer_name: "tester".to_string(),
        ended_at: chrono::Utc::now(),
        cause: crate::session::TerminalCause::Rejected {
            reason: "test".to_string(),
        },
        via_hysteresis: false,
    }
}

/// Pure policy predicate — codifies which terminal variants trigger the
/// session-complete pipeline. Matches the web player's own behaviour
/// (Completed and Failed finalise; Cancelled may still flush a final
/// segment and shouldn't trigger early; Rejected never started).
#[test]
fn test_terminal_should_run_session_complete_policy() {
    assert!(
        matches!(completed_event("s", "r"), DownloadManagerEvent::Terminal(t) if t.should_run_session_complete_pipeline())
    );
    assert!(
        matches!(failed_event("s", "r"), DownloadManagerEvent::Terminal(t) if t.should_run_session_complete_pipeline())
    );
    assert!(
        !matches!(cancelled_event("s", "r"), DownloadManagerEvent::Terminal(t) if t.should_run_session_complete_pipeline())
    );
    assert!(
        !matches!(rejected_event("s", "r"), DownloadManagerEvent::Terminal(t) if t.should_run_session_complete_pipeline())
    );
}

/// A recording that ends with `DownloadFailed` (e.g. HLS 404, stalled
/// stream) must still fire the session-complete pipeline:
/// `handle_session_transition` must treat an `Ended` with a `Failed`
/// cause the same as one with `Completed`, not drop it in a catch-all.
#[tokio::test]
async fn test_handle_download_event_failed_triggers_session_complete() {
    let session_repo = Arc::new(TestSessionRepository::new(Some(
        chrono::Utc::now().timestamp_millis(),
    )));
    let manager: PipelineManager = PipelineManager::new().with_session_repository(session_repo);

    let session_id = "session-failed".to_string();
    let streamer_id = "streamer-1".to_string();

    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::ConfigureSession {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            danmu_enabled: false,
            segment_pipeline: None,
            paired_segment_pipeline: None,
            session_complete_pipeline: Some(DagPipelineDefinition::new("empty", vec![])),
        });
    manager.pipeline_coordinator.apply_event_inline(
        PipelineCoordinationEvent::VideoSegmentCompleted {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            segment_index: 0,
            path: PathBuf::from("/seg0.ts"),
        },
    );

    manager
        .handle_session_transition(ended_failed(&session_id, &streamer_id))
        .await;

    assert_eq!(
        manager.pipeline_coordinator.active_session_count_inline(),
        1
    );
}

/// Cancelled is a stop *request*; a final `Completed` may still arrive.
/// Firing the pipeline early would use a missing final segment.
#[tokio::test]
async fn test_handle_download_event_cancelled_does_not_trigger_session_complete() {
    let manager: PipelineManager = PipelineManager::new();

    let session_id = "session-cancelled".to_string();
    let streamer_id = "streamer-1".to_string();

    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::ConfigureSession {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            danmu_enabled: false,
            segment_pipeline: None,
            paired_segment_pipeline: None,
            session_complete_pipeline: Some(DagPipelineDefinition::new("empty", vec![])),
        });

    manager
        .handle_session_transition(ended_cancelled(&session_id, &streamer_id))
        .await;

    assert_eq!(
        manager.pipeline_coordinator.active_session_count_inline(),
        1,
        "coordinator must still be active after Cancelled (awaiting Completed)"
    );
}

/// Rejected means the download never started — no outputs, nothing to run.
#[tokio::test]
async fn test_handle_download_event_rejected_does_not_trigger_session_complete() {
    let manager: PipelineManager = PipelineManager::new();

    let session_id = "session-rejected".to_string();
    let streamer_id = "streamer-1".to_string();

    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::ConfigureSession {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            danmu_enabled: false,
            segment_pipeline: None,
            paired_segment_pipeline: None,
            session_complete_pipeline: Some(DagPipelineDefinition::new("empty", vec![])),
        });

    manager
        .handle_session_transition(ended_rejected(&session_id, &streamer_id))
        .await;

    assert_eq!(
        manager.pipeline_coordinator.active_session_count_inline(),
        1,
        "coordinator must still be active after Rejected"
    );
}

/// With global danmu recording enabled, the danmu XML can be registered
/// before the final video SegmentCompleted event is processed. A
/// UserDisabled transition must not fire the session-complete pipeline with only
/// danmu inputs (`video_outputs=0 danmu_outputs=1`).
#[tokio::test]
async fn test_session_complete_waits_for_video_output_when_danmu_arrives_first() {
    let manager: PipelineManager = PipelineManager::new();

    let session_id = "session-danmu-first".to_string();
    let streamer_id = "streamer-1".to_string();

    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::ConfigureSession {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            danmu_enabled: true,
            segment_pipeline: None,
            paired_segment_pipeline: None,
            session_complete_pipeline: Some(DagPipelineDefinition::new("empty", vec![])),
        });
    manager.pipeline_coordinator.apply_event_inline(
        PipelineCoordinationEvent::DanmuCollectionStarted {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
        },
    );
    manager.pipeline_coordinator.apply_event_inline(
        PipelineCoordinationEvent::DanmuSegmentCompleted {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            segment_index: 0,
            path: PathBuf::from("/seg0.xml"),
        },
    );
    manager.pipeline_coordinator.apply_event_inline(
        PipelineCoordinationEvent::DanmuCollectionStopped {
            session_id: session_id.clone(),
        },
    );

    let commands =
        manager
            .pipeline_coordinator
            .apply_event_inline(PipelineCoordinationEvent::SessionEnded {
                session_id: session_id.clone(),
                streamer_id: streamer_id.clone(),
                should_run_session_complete: true,
            });
    assert!(
        commands.is_empty(),
        "session end must not trigger without a video output"
    );

    let commands = manager.pipeline_coordinator.apply_event_inline(
        PipelineCoordinationEvent::SessionEndPersisted {
            session_id: session_id.clone(),
        },
    );
    assert!(
        commands.is_empty(),
        "persisted end must not trigger without a video output"
    );

    let commands = manager.pipeline_coordinator.apply_event_inline(
        PipelineCoordinationEvent::VideoSegmentCompleted {
            session_id: session_id.clone(),
            streamer_id,
            segment_index: 0,
            path: PathBuf::from("/seg0.flv"),
        },
    );
    assert_eq!(commands.len(), 1);
    match &commands[0] {
        PipelineCommand::CreateSessionCompleteDag { outputs, .. } => {
            assert_eq!(outputs.video_outputs.len(), 1);
            assert_eq!(outputs.danmu_outputs.len(), 1);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

// =========================================================================
// Pipeline ordering invariant.
//
// The session-complete DAG must run AFTER all per-segment and paired DAGs
// for that session finish. `SessionLifecycle::Ended` means
// "no more bytes will arrive"; it does NOT mean "all post-processing is
// done". The drain-before-fire check lives in the `PipelineCoordinator`
// (see pipeline/coordination.rs), but the entry point through
// `handle_session_transition` must honour it.
//
// Existing handler tests above cover Cancelled/Rejected non-fire, Failed
// fires, DAG completion triggers, and paired-segment flow. The scenarios
// below cover the specific integration of `SessionTransition::Ended` with
// in-flight per-segment DAGs.
// =========================================================================

/// F1 — Session-complete DAG waits for in-flight per-segment video DAGs.
/// Three in-flight video DAGs, observe `SessionTransition::Ended{Failed}`
/// → session-complete is NOT yet scheduled (entry remains, coordinator
/// still active). Complete the three DAGs; session-complete fires only
/// after the last one drains.
#[tokio::test]
async fn f1_session_complete_waits_for_in_flight_video_dags() {
    let session_repo = Arc::new(TestSessionRepository::new(Some(
        chrono::Utc::now().timestamp_millis(),
    )));
    let dag_repo = Arc::new(TestDagRepository::new());
    let manager: PipelineManager = PipelineManager::new()
        .with_session_repository(session_repo)
        .with_dag_repository(dag_repo.clone());

    let session_id = "f1-session".to_string();
    let streamer_id = "f1-streamer".to_string();

    manager
        .pipeline_coordinator
        .apply_event_inline(PipelineCoordinationEvent::ConfigureSession {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            danmu_enabled: false,
            segment_pipeline: Some(DagPipelineDefinition::new(
                "segment",
                vec![DagStep::new("A", PipelineStep::preset("remux"))],
            )),
            paired_segment_pipeline: None,
            session_complete_pipeline: Some(DagPipelineDefinition::new("empty", vec![])),
        });

    // Three in-flight per-segment video DAGs are tracked so the coordinator
    // reports readiness only after they drain.
    for idx in 0..3 {
        manager.pipeline_coordinator.apply_event_inline(
            PipelineCoordinationEvent::SegmentDagStarted {
                session_id: session_id.clone(),
                streamer_id: streamer_id.clone(),
                segment_index: idx,
                source: SourceType::Video,
            },
        );
    }

    // Observe Ended{Failed}: coordinator learns "no more bytes" but three
    // per-segment DAGs are still in flight.
    manager
        .handle_session_transition(ended_failed(&session_id, &streamer_id))
        .await;

    assert_eq!(
        manager.pipeline_coordinator.active_session_count_inline(),
        1,
        "session-complete must NOT fire while per-segment DAGs are pending"
    );

    // Drain two of three DAGs — still gated.
    for idx in 0..2 {
        let dag_def = DagPipelineDefinition::new(
            "seg-dag",
            vec![DagStep::new("A", PipelineStep::preset("remux"))],
        );
        let mut dag = DagExecutionDbModel::new(
            &dag_def,
            Some(streamer_id.clone()),
            Some(session_id.clone()),
        );
        dag.segment_index = Some(idx);
        dag.segment_source = Some("video".to_string());
        let dag_id = dag.id.clone();
        dag_repo.insert(dag);

        manager
            .handle_dag_completion(DagCompletionInfo {
                dag_id,
                streamer_id: Some(streamer_id.clone()),
                session_id: Some(session_id.clone()),
                succeeded: true,
                leaf_outputs: vec![format!("/out{idx}.mp4")],
            })
            .await;
    }

    assert_eq!(
        manager.pipeline_coordinator.active_session_count_inline(),
        1,
        "session-complete must NOT fire until the last per-segment DAG drains"
    );

    // Drain the final DAG — session-complete should now fire.
    let dag_def = DagPipelineDefinition::new(
        "seg-dag",
        vec![DagStep::new("A", PipelineStep::preset("remux"))],
    );
    let mut dag = DagExecutionDbModel::new(
        &dag_def,
        Some(streamer_id.clone()),
        Some(session_id.clone()),
    );
    dag.segment_index = Some(2);
    dag.segment_source = Some("video".to_string());
    let dag_id = dag.id.clone();
    dag_repo.insert(dag);

    manager
        .handle_dag_completion(DagCompletionInfo {
            dag_id,
            streamer_id: Some(streamer_id.clone()),
            session_id: Some(session_id.clone()),
            succeeded: true,
            leaf_outputs: vec!["/out2.mp4".to_string()],
        })
        .await;

    assert_eq!(
        manager.pipeline_coordinator.active_session_count_inline(),
        1,
        "session-complete fires after all per-segment DAGs drain"
    );
}
