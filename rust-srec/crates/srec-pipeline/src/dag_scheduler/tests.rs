use super::*;
use crate::JobFailureOutcome;
use crate::job_queue::parse_job_state;
use srec_core::CoreError;
use srec_sqlite::models::{DagExecutionStatus, DagStepStatus, JobDbModel};

#[tokio::test]
async fn completion_reuses_atomic_snapshots_and_keeps_guarded_downstream_publication() {
    use crate::repositories::{SqlxDagRepository, SqlxJobRepository};
    use srec_sqlite::test_support::SqlTrace;
    let pool = srec_sqlite::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    srec_sqlite::run_migrations(&pool).await.unwrap();
    let dags = Arc::new(SqlxDagRepository::new(pool.clone(), pool.clone()));
    let jobs = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
    let queue = Arc::new(JobQueue::with_repository(Default::default(), jobs.clone()));
    let scheduler = DagScheduler::new(queue, dags.clone(), jobs.clone());
    let created = scheduler
        .create_dag_pipeline(
            two_step_pipeline("atomic snapshots"),
            &["input.flv".to_owned()],
            DagRunContext::default(),
        )
        .await
        .unwrap();
    let steps = dags.get_steps_by_dag(&created.dag_id).await.unwrap();
    let first = steps.iter().find(|step| step.step_id == "A").unwrap();
    let trace = SqlTrace::install(&pool).await;
    let update = scheduler
        .on_job_completed(
            &first.id,
            &["first.mp4".to_owned()],
            Some("Streamer"),
            Some("Title"),
            Some("twitch"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(update.new_job_ids.len(), 1);
    assert!(update.completion.is_none());
    // One row fetch decides readiness inside the transaction; the only
    // other read is the DAG reload for the completion check.
    let statements = trace.statements();
    assert_eq!(
        statements
            .iter()
            .filter(|sql| sql.starts_with("SELECT "))
            .count(),
        2,
        "{statements:?}"
    );
    assert_eq!(
        statements
            .iter()
            .filter(|sql| sql.starts_with("SELECT * FROM dag_execution WHERE id ="))
            .count(),
        1
    );
    assert!(
        !statements
            .iter()
            .any(|sql| sql.starts_with("SELECT * FROM dag_step_execution WHERE id ="))
    );
    assert!(statements.iter().any(|sql| {
        sql.starts_with("UPDATE dag_step_execution SET status = 'PROCESSING'")
            && sql.contains("AND job_id IS NULL")
            && sql.contains("SELECT status FROM dag_execution")
    }));
    let replay = scheduler
        .on_job_completed(&first.id, &["wrong.mp4".to_owned()], None, None, None, None)
        .await
        .unwrap();
    assert!(replay.new_job_ids.is_empty());
    assert_eq!(
        dags.get_dag(&created.dag_id).await.unwrap().completed_steps,
        1
    );
    let downstream = jobs.get_job(&update.new_job_ids[0]).await.unwrap();
    let state: serde_json::Value = serde_json::from_str(&downstream.state).unwrap();
    assert_eq!(state["streamer_name"], "Streamer");
    assert_eq!(state["session_title"], "Title");
    assert_eq!(state["platform"], "twitch");
    let second = downstream.dag_step_execution_id.unwrap();
    for output in ["final.mp4", "duplicate-must-not-replace.mp4"] {
        let update = scheduler
            .on_job_completed(&second, &[output.to_owned()], None, None, None, None)
            .await
            .unwrap();
        let completion = update.completion.unwrap();
        assert!(completion.succeeded);
        assert_eq!(completion.dag_id, created.dag_id);
        assert_eq!(completion.leaf_outputs, vec!["final.mp4"]);
    }
    assert_eq!(
        dags.get_dag(&created.dag_id).await.unwrap().completed_steps,
        2
    );
    pool.close().await;
}

#[test]
fn shared_leaf_collection_reports_incomplete_data_without_losing_valid_outputs() {
    let definition = DagPipelineDefinition::new(
        "leaves",
        vec![
            DagStep::new(
                "missing",
                PipelineStep::inline("remux", serde_json::json!({})),
            ),
            DagStep::new(
                "corrupt",
                PipelineStep::inline("remux", serde_json::json!({})),
            ),
            DagStep::new("good", PipelineStep::inline("remux", serde_json::json!({}))),
        ],
    );
    let mut corrupt = DagStepExecutionDbModel::new("dag", "corrupt", &[]);
    corrupt.outputs = Some("not JSON".to_owned());
    let mut good = DagStepExecutionDbModel::new("dag", "good", &[]);
    good.set_outputs(&[
        "A.mp4".to_owned(),
        "a.mp4".to_owned(),
        "last.mp4".to_owned(),
    ]);
    let collected =
        DagScheduler::collect_leaf_outputs_from_step_executions(&definition, &[good, corrupt]);
    assert!(!collected.complete);
    // Windows and the default macOS file systems fold case, so the two
    // spellings are one file there (see `utils::fs::path_dedup_key`).
    assert_eq!(
        collected.paths,
        if cfg!(any(windows, target_os = "macos")) {
            vec!["A.mp4", "last.mp4"]
        } else {
            vec!["A.mp4", "a.mp4", "last.mp4"]
        }
    );
}
use crate::JobQueue;
use crate::repositories::dag::DagRepository;
use crate::repositories::job::JobRepository;
use srec_sqlite::models::dag::DagStepExecutionDbModel;
use srec_sqlite::models::{DagPipelineDefinition, DagStep, JobStatus, PipelineStep};
use std::sync::Arc;
use tempfile::TempDir;

async fn setup_test_pool() -> sqlx::SqlitePool {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("dag_scheduler_test.db");
    let db_url = format!("sqlite:{}?mode=rwc", db_path.to_string_lossy());
    let pool = srec_sqlite::init_pool(&db_url).await.unwrap();
    srec_sqlite::run_migrations(&pool).await.unwrap();
    std::mem::forget(dir);
    pool
}

struct NoopJobRepository;

#[async_trait::async_trait]
impl JobRepository for NoopJobRepository {
    async fn list_unclaimable_pending_jobs(
        &self,
        _supported_types: &[&str],
        _after_id: Option<&str>,
        _limit: u32,
    ) -> srec_sqlite::DbResult<Vec<crate::repositories::UnclaimableJob>> {
        unimplemented!("not needed for these tests")
    }

    async fn create_job(
        &self,
        _job: &srec_sqlite::models::JobDbModel,
    ) -> srec_sqlite::DbResult<()> {
        unimplemented!("not needed for these tests")
    }

    async fn get_job(&self, _id: &str) -> srec_sqlite::DbResult<srec_sqlite::models::JobDbModel> {
        unimplemented!("not needed for these tests")
    }

    async fn list_jobs_by_status(
        &self,
        _status: JobStatus,
    ) -> srec_sqlite::DbResult<Vec<srec_sqlite::models::JobDbModel>> {
        unimplemented!("not needed for these tests")
    }

    async fn update_job_status(&self, _id: &str, _status: JobStatus) -> srec_sqlite::DbResult<()> {
        unimplemented!("not needed for these tests")
    }

    async fn mark_job_failed(
        &self,
        _id: &str,
        _error: &str,
        _retry_after: Option<i64>,
    ) -> srec_sqlite::DbResult<u64> {
        unimplemented!("not needed for these tests")
    }

    async fn mark_job_cancelled(&self, _id: &str) -> srec_sqlite::DbResult<u64> {
        unimplemented!("not needed for these tests")
    }

    async fn reset_job_for_retry(&self, _id: &str) -> srec_sqlite::DbResult<()> {
        unimplemented!("not needed for these tests")
    }

    async fn count_pending_jobs(
        &self,
        _job_types: Option<&[String]>,
    ) -> srec_sqlite::DbResult<u64> {
        unimplemented!("not needed for these tests")
    }

    async fn count_pending_jobs_by_type(&self) -> srec_sqlite::DbResult<Vec<(String, u64)>> {
        unimplemented!("not needed for these tests")
    }

    async fn claim_next_pending_job(
        &self,
        _job_types: Option<&[String]>,
    ) -> srec_sqlite::DbResult<Option<srec_sqlite::models::JobDbModel>> {
        unimplemented!("not needed for these tests")
    }

    async fn get_job_execution_info(&self, _id: &str) -> srec_sqlite::DbResult<Option<String>> {
        unimplemented!("not needed for these tests")
    }

    async fn update_job_execution_info(
        &self,
        _id: &str,
        _execution_info: &str,
    ) -> srec_sqlite::DbResult<()> {
        unimplemented!("not needed for these tests")
    }

    async fn update_job_state(&self, _id: &str, _state: &str) -> srec_sqlite::DbResult<()> {
        unimplemented!("not needed for these tests")
    }

    async fn update_job(
        &self,
        _job: &srec_sqlite::models::JobDbModel,
    ) -> srec_sqlite::DbResult<()> {
        unimplemented!("not needed for these tests")
    }

    async fn update_job_if_status(
        &self,
        _job: &srec_sqlite::models::JobDbModel,
        _expected_status: JobStatus,
    ) -> srec_sqlite::DbResult<u64> {
        unimplemented!("not needed for these tests")
    }

    async fn reset_processing_jobs(&self) -> srec_sqlite::DbResult<i32> {
        unimplemented!("not needed for these tests")
    }

    async fn delete_job(&self, _id: &str) -> srec_sqlite::DbResult<()> {
        unimplemented!("not needed for these tests")
    }

    async fn add_execution_log(
        &self,
        _log: &srec_sqlite::models::JobExecutionLogDbModel,
    ) -> srec_sqlite::DbResult<()> {
        unimplemented!("not needed for these tests")
    }

    async fn add_execution_logs(
        &self,
        _logs: &[srec_sqlite::models::JobExecutionLogDbModel],
    ) -> srec_sqlite::DbResult<()> {
        unimplemented!("not needed for these tests")
    }

    async fn list_execution_logs(
        &self,
        _job_id: &str,
        _pagination: &srec_sqlite::models::Pagination,
    ) -> srec_sqlite::DbResult<(Vec<srec_sqlite::models::JobExecutionLogDbModel>, u64)> {
        Ok((Vec::new(), 0))
    }

    async fn delete_execution_logs_for_job(&self, _job_id: &str) -> srec_sqlite::DbResult<()> {
        unimplemented!("not needed for these tests")
    }

    async fn list_pending_jobs(
        &self,
        _limit: u32,
    ) -> srec_sqlite::DbResult<(Vec<srec_sqlite::models::JobDbModel>, u64)> {
        unimplemented!("not needed for these tests")
    }

    async fn count_jobs_by_status(
        &self,
        _status: srec_sqlite::models::JobStatus,
    ) -> srec_sqlite::DbResult<u64> {
        unimplemented!("not needed for these tests")
    }

    async fn count_unfinished_jobs_for_streamer(
        &self,
        _streamer_id: &str,
    ) -> srec_sqlite::DbResult<u64> {
        unimplemented!("not needed for these tests")
    }

    async fn count_unfinished_jobs_for_session(
        &self,
        _session_id: &str,
    ) -> srec_sqlite::DbResult<u64> {
        unimplemented!("not needed for these tests")
    }

    async fn get_job_counts_by_status(
        &self,
    ) -> srec_sqlite::DbResult<srec_sqlite::models::JobCounts> {
        unimplemented!("not needed for these tests")
    }

    async fn get_avg_processing_time(&self) -> srec_sqlite::DbResult<Option<f64>> {
        unimplemented!("not needed for these tests")
    }

    async fn cancel_jobs_by_pipeline(&self, _pipeline_id: &str) -> srec_sqlite::DbResult<u64> {
        unimplemented!("not needed for these tests")
    }

    async fn get_jobs_by_pipeline(
        &self,
        _pipeline_id: &str,
    ) -> srec_sqlite::DbResult<Vec<srec_sqlite::models::JobDbModel>> {
        unimplemented!("not needed for these tests")
    }

    async fn delete_jobs_by_pipeline(&self, _pipeline_id: &str) -> srec_sqlite::DbResult<u64> {
        unimplemented!("not needed for these tests")
    }
}

#[test]
fn test_collect_leaf_outputs_is_deterministic_and_deduped() {
    // Graph: A -> B, A -> C (leaves are B, C)
    // Definition order is A, B, C so outputs should be B then C, regardless of DB row order.
    let def = DagPipelineDefinition::new(
        "test",
        vec![
            DagStep {
                id: "A".to_string(),
                step: PipelineStep::inline("noop", serde_json::json!({})),
                depends_on: vec![],
                retry: None,
                timeout_secs: None,
            },
            DagStep {
                id: "B".to_string(),
                step: PipelineStep::inline("noop", serde_json::json!({})),
                depends_on: vec!["A".to_string()],
                retry: None,
                timeout_secs: None,
            },
            DagStep {
                id: "C".to_string(),
                step: PipelineStep::inline("noop", serde_json::json!({})),
                depends_on: vec!["A".to_string()],
                retry: None,
                timeout_secs: None,
            },
        ],
    );

    let mut exec_b = DagStepExecutionDbModel::new("dag1", "B", &["A".to_string()]);
    exec_b.set_outputs(&["x".to_string(), "y".to_string()]);
    let mut exec_c = DagStepExecutionDbModel::new("dag1", "C", &["A".to_string()]);
    exec_c.set_outputs(&["y".to_string(), "z".to_string()]);

    // Simulate non-deterministic row order from DB (C then B).
    let step_execs = vec![exec_c, exec_b];

    let collected = DagScheduler::collect_leaf_outputs_from_step_executions(&def, &step_execs);
    assert!(collected.complete);
    let out = collected.paths;
    assert_eq!(out, vec!["x".to_string(), "y".to_string(), "z".to_string()]);
}

#[tokio::test]
async fn test_dag_jobs_persist_session_start() {
    let pool = setup_test_pool().await;
    let dag_repo = Arc::new(crate::repositories::dag::SqlxDagRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let job_repo = Arc::new(crate::repositories::job::SqlxJobRepository::new(
        pool.clone(),
        pool,
    ));
    let scheduler = DagScheduler::new(
        Arc::new(JobQueue::with_repository(
            Default::default(),
            job_repo.clone(),
        )),
        dag_repo,
        job_repo.clone(),
    );
    let session_start = jiff::fmt::temporal::DateTimeParser::new()
        .parse_timestamp("2024-01-01T23:30:00Z")
        .unwrap();
    let dag_def = DagPipelineDefinition::new(
        "session start propagation",
        vec![
            DagStep {
                id: "A".to_string(),
                step: PipelineStep::inline("noop", serde_json::json!({})),
                depends_on: vec![],
                retry: None,
                timeout_secs: None,
            },
            DagStep {
                id: "B".to_string(),
                step: PipelineStep::inline("noop", serde_json::json!({})),
                depends_on: vec!["A".to_string()],
                retry: None,
                timeout_secs: None,
            },
        ],
    );

    let created = scheduler
        .create_dag_pipeline(
            dag_def,
            &["/input.flv".to_string()],
            DagRunContext {
                streamer_id: Some("streamer-1".to_string()),
                session_id: Some("session-1".to_string()),
                streamer_name: Some("Streamer".to_string()),
                session_title: Some("Title".to_string()),
                platform: Some("Platform".to_string()),
                session_start: Some(session_start),
                manifest: None,
            },
        )
        .await
        .unwrap();

    let root_job = job_repo.get_job(&created.root_job_ids[0]).await.unwrap();
    let root_state: serde_json::Value = serde_json::from_str(&root_job.state).unwrap();
    assert_eq!(
        root_state
            .get("session_start_ms")
            .and_then(|value| value.as_i64()),
        Some(session_start.as_millisecond())
    );

    let update = scheduler
        .on_job_completed(
            root_job.dag_step_execution_id.as_deref().unwrap(),
            &["/tmp/a.mp4".to_string()],
            Some("Streamer"),
            Some("Title"),
            Some("Platform"),
            Some(session_start),
        )
        .await
        .unwrap();
    assert_eq!(update.new_job_ids.len(), 1);

    let downstream_job = job_repo.get_job(&update.new_job_ids[0]).await.unwrap();
    let downstream_state: serde_json::Value = serde_json::from_str(&downstream_job.state).unwrap();
    assert_eq!(
        downstream_state
            .get("session_start_ms")
            .and_then(|value| value.as_i64()),
        Some(session_start.as_millisecond())
    );
}

/// Deleting a DAG removes job rows outright, so a job a worker is still running has to be
/// told to stop and everything the queue holds for it has to go with the rows.
#[tokio::test]
async fn test_delete_dag_stands_down_active_jobs() {
    let pool = setup_test_pool().await;
    let dag_repo = Arc::new(crate::repositories::dag::SqlxDagRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let job_repo = Arc::new(crate::repositories::job::SqlxJobRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let job_queue = Arc::new(JobQueue::with_repository(
        Default::default(),
        job_repo.clone(),
    ));
    let scheduler = DagScheduler::new(job_queue.clone(), dag_repo, job_repo.clone());
    let dag_def = DagPipelineDefinition::new(
        "delete stands down jobs",
        vec![DagStep {
            id: "A".to_string(),
            step: PipelineStep::inline("noop", serde_json::json!({})),
            depends_on: vec![],
            retry: None,
            timeout_secs: None,
        }],
    );

    let created = scheduler
        .create_dag_pipeline(
            dag_def,
            &["/input.flv".to_string()],
            DagRunContext::default(),
        )
        .await
        .unwrap();

    // Stands in for a worker claiming the job: dequeue is what registers its token.
    let (claimed, _) = job_queue
        .dequeue(None)
        .await
        .unwrap()
        .expect("the root job is claimable");
    assert_eq!(claimed.id, created.root_job_ids[0]);
    let token = job_queue
        .get_cancellation_token(&claimed.id)
        .await
        .expect("dequeue registers a cancellation token");
    assert!(!token.is_cancelled());
    assert_eq!(job_queue.depth(), 1);

    scheduler.delete_dag(&created.dag_id).await.unwrap();

    assert!(
        token.is_cancelled(),
        "a worker inside Processor::process must be told the job is gone"
    );
    assert_eq!(
        job_queue.depth(),
        0,
        "the deleted job still counted as queued"
    );
    assert!(job_queue.get_job(&claimed.id).await.unwrap().is_none());
    assert!(
        job_queue
            .get_cancellation_token(&claimed.id)
            .await
            .is_none()
    );
    assert!(job_repo.get_job(&claimed.id).await.is_err());
}

/// The retry fan-out path (reset_dag_for_retry -> enqueue_now_ready_steps)
/// has no session context of its own; jobs it creates must recover the
/// placeholder metadata from a sibling step's persisted job state.
#[tokio::test]
async fn test_retry_fanout_recovers_metadata_from_sibling_job() {
    let pool = setup_test_pool().await;
    let dag_repo = Arc::new(crate::repositories::dag::SqlxDagRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let job_repo = Arc::new(crate::repositories::job::SqlxJobRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let scheduler = DagScheduler::new(
        Arc::new(JobQueue::with_repository(
            Default::default(),
            job_repo.clone(),
        )),
        dag_repo,
        job_repo.clone(),
    );
    let session_start = jiff::fmt::temporal::DateTimeParser::new()
        .parse_timestamp("2024-01-01T23:30:00Z")
        .unwrap();
    let dag_def = DagPipelineDefinition::new(
        "retry fanout metadata recovery",
        vec![
            DagStep {
                id: "A".to_string(),
                step: PipelineStep::inline("noop", serde_json::json!({})),
                depends_on: vec![],
                retry: None,
                timeout_secs: None,
            },
            DagStep {
                id: "B".to_string(),
                step: PipelineStep::inline("noop", serde_json::json!({})),
                depends_on: vec!["A".to_string()],
                retry: None,
                timeout_secs: None,
            },
        ],
    );

    let created = scheduler
        .create_dag_pipeline(
            dag_def,
            &["/input.flv".to_string()],
            DagRunContext {
                streamer_id: Some("streamer-1".to_string()),
                session_id: Some("session-1".to_string()),
                streamer_name: Some("Streamer".to_string()),
                session_title: Some("Title".to_string()),
                platform: Some("Platform".to_string()),
                session_start: Some(session_start),
                manifest: None,
            },
        )
        .await
        .unwrap();

    // Put the DAG in the shape reset_dag_for_retry produces for a
    // fail-fast cancelled parallel branch: dependency completed with
    // outputs, dependent step BLOCKED with no job row attached.
    let steps = scheduler.get_dag_steps(&created.dag_id).await.unwrap();
    let step_a = steps.iter().find(|s| s.step_id == "A").unwrap();
    sqlx::query("UPDATE dag_step_execution SET status = ?, outputs = ? WHERE id = ?")
        .bind(DagStepStatus::Completed.as_str())
        .bind(r#"["/tmp/a.mp4"]"#)
        .bind(&step_a.id)
        .execute(&pool)
        .await
        .unwrap();

    let new_job_ids = scheduler
        .enqueue_now_ready_steps(&created.dag_id)
        .await
        .unwrap()
        .new_job_ids;
    assert_eq!(new_job_ids.len(), 1);

    let recovered_job = job_repo.get_job(&new_job_ids[0]).await.unwrap();
    let state: serde_json::Value = serde_json::from_str(&recovered_job.state).unwrap();
    assert_eq!(
        state.get("session_start_ms").and_then(|v| v.as_i64()),
        Some(session_start.as_millisecond())
    );
    assert_eq!(
        state.get("streamer_name").and_then(|v| v.as_str()),
        Some("Streamer")
    );
}

/// Every path that materializes a step job — publication, completion of a
/// dependency, retry fan-out and startup recovery — must copy the DAG row's
/// manifest into the job so the processor can pair videos with danmu.
#[tokio::test]
async fn manifest_reaches_jobs_created_by_every_materialization_path() {
    use crate::manifest::{ManifestScope, ManifestSegment, PipelineInputManifest};
    let pool = setup_test_pool().await;
    let dag_repo = Arc::new(crate::repositories::dag::SqlxDagRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let job_repo = Arc::new(crate::repositories::job::SqlxJobRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let scheduler = DagScheduler::new(
        Arc::new(JobQueue::with_repository(
            Default::default(),
            job_repo.clone(),
        )),
        dag_repo.clone(),
        job_repo.clone(),
    );
    let manifest = PipelineInputManifest::new(
        "session-1",
        "streamer-1",
        ManifestScope::Segment { index: 4 },
        vec![ManifestSegment {
            segment_index: 4,
            video: vec!["/rec/4.mp4".to_string()],
            danmu: vec!["/rec/4.xml".to_string()],
        }],
    );
    let manifest_value = serde_json::to_value(&manifest).unwrap();
    let state_manifest = |job: &JobDbModel| -> serde_json::Value {
        let state: serde_json::Value = serde_json::from_str(&job.state).unwrap();
        state["manifest"].clone()
    };

    let created = scheduler
        .create_dag_pipeline_with_hook(
            two_step_pipeline("manifest propagation"),
            &["/rec/4.mp4".to_string(), "/rec/4.xml".to_string()],
            DagRunContext {
                streamer_id: Some("streamer-1".to_string()),
                session_id: Some("session-1".to_string()),
                streamer_name: Some("Streamer".to_string()),
                ..DagRunContext::default()
            },
            Some(DagExecutionMetadata {
                segment_index: Some(4),
                segment_source: Some(DagSegmentSource::Paired),
                manifest: Some(manifest.clone()),
            }),
            None,
        )
        .await
        .unwrap();

    let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
    assert_eq!(
        input_manifest(&dag).as_deref(),
        Some(&manifest),
        "publication stores the manifest on the DAG row"
    );
    let root = job_repo.get_job(&created.root_job_ids[0]).await.unwrap();
    assert_eq!(state_manifest(&root), manifest_value, "root job");
    let parsed = parse_job_state(&root.state);
    assert_eq!(parsed.manifest.as_deref(), Some(&manifest));
    assert_eq!(parsed.streamer_name.as_deref(), Some("Streamer"));

    // Completion of A creates B from the DAG row, not from A's job state.
    let update = scheduler
        .on_job_completed(
            root.dag_step_execution_id.as_deref().unwrap(),
            &["/rec/4.mp4".to_string(), "/rec/4.ass".to_string()],
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(update.new_job_ids.len(), 1);
    let downstream = job_repo.get_job(&update.new_job_ids[0]).await.unwrap();
    assert_eq!(state_manifest(&downstream), manifest_value, "completion");

    // Retry fan-out: B is BLOCKED again without a job row.
    let step_b = dag_repo
        .get_step(downstream.dag_step_execution_id.as_deref().unwrap())
        .await
        .unwrap();
    sqlx::query("UPDATE dag_step_execution SET status = 'BLOCKED', job_id = NULL WHERE id = ?")
        .bind(&step_b.id)
        .execute(&pool)
        .await
        .unwrap();
    let retried = scheduler
        .enqueue_now_ready_steps(&created.dag_id)
        .await
        .unwrap()
        .new_job_ids;
    assert_eq!(retried.len(), 1);
    let retried_job = job_repo.get_job(&retried[0]).await.unwrap();
    assert_eq!(
        state_manifest(&retried_job),
        manifest_value,
        "retry fan-out"
    );

    // Startup recovery materializes a PENDING step through the same row once
    // no live job references the step any more.
    sqlx::query("UPDATE job SET status = 'CANCELLED' WHERE dag_step_execution_id = ?")
        .bind(&step_b.id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE dag_step_execution SET status = 'PENDING', job_id = NULL WHERE id = ?")
        .bind(&step_b.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 1);
    let recovered_step = dag_repo.get_step(&step_b.id).await.unwrap();
    let recovered_job = job_repo
        .get_job(recovered_step.job_id.as_deref().unwrap())
        .await
        .unwrap();
    assert_eq!(state_manifest(&recovered_job), manifest_value, "recovery");

    // A DAG without a manifest yields jobs without one.
    let plain = scheduler
        .create_dag_pipeline(
            two_step_pipeline("no manifest"),
            &["/input.flv".to_string()],
            DagRunContext::default(),
        )
        .await
        .unwrap();
    let plain_root = job_repo.get_job(&plain.root_job_ids[0]).await.unwrap();
    assert!(parse_job_state(&plain_root.state).manifest.is_none());
    assert!(input_manifest(&dag_repo.get_dag(&plain.dag_id).await.unwrap()).is_none());
}

/// Failing a DAG that is already terminal changes nothing and yields no
/// completion, so a retry that could not even reset the rows does not replay
/// the earlier outcome to the coordinator.
#[tokio::test]
async fn fail_dag_leaves_a_terminal_dag_alone() {
    let pool = setup_test_pool().await;
    let dag_repo = Arc::new(crate::repositories::dag::SqlxDagRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let job_repo = Arc::new(crate::repositories::job::SqlxJobRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let scheduler = DagScheduler::new(
        Arc::new(JobQueue::with_repository(
            Default::default(),
            job_repo.clone(),
        )),
        dag_repo.clone(),
        job_repo,
    );
    let created = scheduler
        .create_dag_pipeline(
            two_step_pipeline("terminal fail"),
            &["/input.flv".to_string()],
            DagRunContext::default(),
        )
        .await
        .unwrap();
    let first = scheduler
        .fail_dag(&created.dag_id, "first failure")
        .await
        .unwrap();
    assert!(first.is_some_and(|completion| !completion.succeeded));

    let second = scheduler
        .fail_dag(&created.dag_id, "retry failed")
        .await
        .unwrap();
    assert!(second.is_none());
    let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
    assert_eq!(dag.get_status(), Some(DagExecutionStatus::Failed));
    assert_eq!(dag.error.as_deref(), Some("first failure"));
}

/// A PROCESSING step whose job row is gone can never be reported on; startup
/// reconciliation fails it like a step whose job failed.
#[tokio::test]
async fn startup_reconciliation_fails_a_processing_step_without_a_job_row() {
    let pool = setup_test_pool().await;
    let dag_repo = Arc::new(crate::repositories::dag::SqlxDagRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let job_repo = Arc::new(crate::repositories::job::SqlxJobRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let scheduler = DagScheduler::new(
        Arc::new(JobQueue::with_repository(
            Default::default(),
            job_repo.clone(),
        )),
        dag_repo.clone(),
        job_repo.clone(),
    );
    let created = scheduler
        .create_dag_pipeline(
            two_step_pipeline("jobless processing step"),
            &["/input.flv".to_string()],
            DagRunContext::default(),
        )
        .await
        .unwrap();
    // Deleting the job row nulls the step's job_id through the foreign key.
    job_repo.delete_job(&created.root_job_ids[0]).await.unwrap();
    let steps = dag_repo.get_steps_by_dag(&created.dag_id).await.unwrap();
    let step_a = steps.iter().find(|step| step.step_id == "A").unwrap();
    assert_eq!(step_a.get_status(), Some(DagStepStatus::Processing));
    assert!(step_a.job_id.is_none());

    assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 0);
    let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
    assert_eq!(dag.get_status(), Some(DagExecutionStatus::Failed));
    assert!(
        dag.error
            .as_deref()
            .unwrap()
            .contains("job row no longer exists"),
        "{:?}",
        dag.error
    );
}

/// Retry fan-out and startup recovery decide readiness from rows a live
/// completion may have advanced in between; a step that already has a job
/// is skipped instead of failing the whole DAG.
#[tokio::test]
async fn already_materialized_step_does_not_fail_the_dag() {
    let pool = setup_test_pool().await;
    let dag_repo = Arc::new(crate::repositories::dag::SqlxDagRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let job_repo = Arc::new(crate::repositories::job::SqlxJobRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let scheduler = DagScheduler::new(
        Arc::new(JobQueue::with_repository(
            Default::default(),
            job_repo.clone(),
        )),
        dag_repo.clone(),
        job_repo.clone(),
    );
    let created = scheduler
        .create_dag_pipeline(
            two_step_pipeline("materialization race"),
            &["/input.flv".to_string()],
            DagRunContext::default(),
        )
        .await
        .unwrap();
    let steps = dag_repo.get_steps_by_dag(&created.dag_id).await.unwrap();
    let step_a = steps.iter().find(|step| step.step_id == "A").unwrap();
    let step_b = steps.iter().find(|step| step.step_id == "B").unwrap();
    // Another materialization attached a job to B before this completion
    // got to it.
    let mut elsewhere = JobDbModel::new_pipeline_step("noop", "[]", "[]", 0, None, None);
    elsewhere.pipeline_id = Some(created.dag_id.clone());
    elsewhere.dag_step_execution_id = Some(step_b.id.clone());
    job_repo.create_job(&elsewhere).await.unwrap();
    sqlx::query("UPDATE dag_step_execution SET job_id = ? WHERE id = ?")
        .bind(&elsewhere.id)
        .bind(&step_b.id)
        .execute(&pool)
        .await
        .unwrap();

    let update = scheduler
        .on_job_completed(&step_a.id, &["/a.mp4".to_string()], None, None, None, None)
        .await
        .unwrap();
    assert!(update.new_job_ids.is_empty());
    assert!(update.completion.is_none());
    let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
    assert_eq!(dag.get_status(), Some(DagExecutionStatus::Processing));
    assert_eq!(dag.error, None);
    assert_eq!(
        dag_repo.get_step(&step_b.id).await.unwrap().job_id,
        Some(elsewhere.id)
    );
}

/// A worker that persisted its job's failure but never reported it leaves the
/// step PROCESSING; startup reconciliation fails the DAG so it can be retried.
#[tokio::test]
async fn startup_reconciliation_fails_a_step_whose_job_already_failed() {
    let pool = setup_test_pool().await;
    let dag_repo = Arc::new(crate::repositories::dag::SqlxDagRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let job_repo = Arc::new(crate::repositories::job::SqlxJobRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let scheduler = DagScheduler::new(
        Arc::new(JobQueue::with_repository(
            Default::default(),
            job_repo.clone(),
        )),
        dag_repo.clone(),
        job_repo.clone(),
    );
    let created = scheduler
        .create_dag_pipeline(
            two_step_pipeline("stranded failure"),
            &["/input.flv".to_string()],
            DagRunContext::default(),
        )
        .await
        .unwrap();
    job_repo
        .mark_job_failed(&created.root_job_ids[0], "disk full", None)
        .await
        .unwrap();

    assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 0);
    let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
    assert_eq!(dag.get_status(), Some(DagExecutionStatus::Failed));
    assert!(
        dag.error.as_deref().unwrap().contains("disk full"),
        "{:?}",
        dag.error
    );
    let mut steps = dag_repo.get_steps_by_dag(&created.dag_id).await.unwrap();
    steps.sort_by(|left, right| left.step_id.cmp(&right.step_id));
    assert_eq!(
        steps
            .iter()
            .map(|step| (step.step_id.as_str(), step.status.as_str()))
            .collect::<Vec<_>>(),
        vec![("A", "FAILED"), ("B", "CANCELLED")]
    );
    assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 0);
    assert!(
        dag_repo
            .list_processing_steps_with_failed_jobs()
            .await
            .unwrap()
            .is_empty()
    );
}

fn two_step_pipeline(name: &str) -> DagPipelineDefinition {
    DagPipelineDefinition::new(
        name,
        vec![
            DagStep {
                id: "A".to_string(),
                step: PipelineStep::inline("noop", serde_json::json!({})),
                depends_on: vec![],
                retry: None,
                timeout_secs: None,
            },
            DagStep {
                id: "B".to_string(),
                step: PipelineStep::inline("noop", serde_json::json!({})),
                depends_on: vec!["A".to_string()],
                retry: None,
                timeout_secs: None,
            },
        ],
    )
}

/// `noop` steps wired as given: `(id, dependencies)`.
fn noop_pipeline(name: &str, steps: &[(&str, &[&str])]) -> DagPipelineDefinition {
    DagPipelineDefinition::new(
        name,
        steps
            .iter()
            .map(|(id, depends_on)| {
                DagStep::with_dependencies(
                    *id,
                    PipelineStep::inline("noop", serde_json::json!({})),
                    depends_on.iter().map(|dep| dep.to_string()).collect(),
                )
            })
            .collect(),
    )
}

/// `"<step>=<status>"` for every step of a DAG, in step-id order.
async fn step_statuses(
    dag_repo: &crate::repositories::dag::SqlxDagRepository,
    dag_id: &str,
) -> Vec<String> {
    let mut statuses: Vec<String> = dag_repo
        .get_steps_by_dag(dag_id)
        .await
        .unwrap()
        .into_iter()
        .map(|step| format!("{}={}", step.step_id, step.status))
        .collect();
    statuses.sort();
    statuses
}

async fn complete(
    scheduler: &DagScheduler,
    step_execution_id: &str,
    outputs: &[&str],
) -> DagJobCompletedUpdate {
    let outputs: Vec<String> = outputs.iter().map(|output| output.to_string()).collect();
    scheduler
        .on_job_completed(step_execution_id, &outputs, None, None, None, None)
        .await
        .unwrap()
}

#[tokio::test]
async fn startup_recovery_materializes_pending_jobless_step_once() {
    let pool = setup_test_pool().await;
    let dag_repo = Arc::new(crate::repositories::dag::SqlxDagRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let job_repo = Arc::new(crate::repositories::job::SqlxJobRepository::new(
        pool.clone(),
        pool,
    ));
    let scheduler = DagScheduler::new(
        Arc::new(JobQueue::with_repository(
            Default::default(),
            job_repo.clone(),
        )),
        dag_repo.clone(),
        job_repo.clone(),
    );
    let created = scheduler
        .create_dag_pipeline(
            two_step_pipeline("recover pending intent"),
            &["/input.flv".to_string()],
            DagRunContext {
                streamer_name: Some("Streamer".to_string()),
                session_title: Some("Title".to_string()),
                ..DagRunContext::default()
            },
        )
        .await
        .unwrap();
    let steps = scheduler.get_dag_steps(&created.dag_id).await.unwrap();
    let step_a = steps.iter().find(|step| step.step_id == "A").unwrap();

    let ready = dag_repo
        .complete_step_and_check_dependents(
            &step_a.id,
            &["/a.mp4".to_string(), "/shared.xml".to_string()],
        )
        .await
        .unwrap()
        .ready_steps;
    assert_eq!(ready.len(), 1);
    let pending = dag_repo.get_step(&ready[0].step.id).await.unwrap();
    assert_eq!(pending.get_status(), Some(DagStepStatus::Pending));
    assert!(pending.job_id.is_none());

    assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 1);
    assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 0);

    let recovered = dag_repo.get_step(&pending.id).await.unwrap();
    assert_eq!(recovered.get_status(), Some(DagStepStatus::Processing));
    let recovered_job = job_repo
        .get_job(recovered.job_id.as_deref().unwrap())
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Vec<String>>(recovered_job.input.as_deref().unwrap()).unwrap(),
        vec!["/a.mp4".to_string(), "/shared.xml".to_string()]
    );
    let state: serde_json::Value = serde_json::from_str(&recovered_job.state).unwrap();
    assert_eq!(
        state.get("streamer_name").and_then(|value| value.as_str()),
        Some("Streamer")
    );
    assert_eq!(
        job_repo
            .get_jobs_by_pipeline(&created.dag_id)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn startup_recovery_advances_reverse_linked_completed_job_without_duplicate() {
    let pool = setup_test_pool().await;
    let dag_repo = Arc::new(crate::repositories::dag::SqlxDagRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let job_repo = Arc::new(crate::repositories::job::SqlxJobRepository::new(
        pool.clone(),
        pool,
    ));
    let scheduler = DagScheduler::new(
        Arc::new(JobQueue::with_repository(
            Default::default(),
            job_repo.clone(),
        )),
        dag_repo.clone(),
        job_repo.clone(),
    );
    let definition = two_step_pipeline("recover reverse link");
    let created = scheduler
        .create_dag_pipeline(
            definition.clone(),
            &["/input.flv".to_string()],
            DagRunContext::default(),
        )
        .await
        .unwrap();
    let steps = scheduler.get_dag_steps(&created.dag_id).await.unwrap();
    let step_a = steps.iter().find(|step| step.step_id == "A").unwrap();
    let step_b = steps.iter().find(|step| step.step_id == "B").unwrap();
    dag_repo
        .complete_step_and_check_dependents(step_a.id.as_str(), &["/a.mp4".to_string()])
        .await
        .unwrap();

    let (mut orphan, _) = DagScheduler::build_step_job(
        &created.dag_id,
        &step_b.id,
        definition.get_step("B").unwrap(),
        vec!["/a.mp4".to_string()],
        &DagRunContext::default(),
    )
    .unwrap();
    orphan.status = JobStatus::Completed.as_str().to_string();
    orphan.set_outputs(&["/b.mp4".to_string()]);
    orphan.completed_at = Some(srec_sqlite::time::now_ms());
    job_repo.create_job(&orphan).await.unwrap();

    assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 0);
    assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 0);
    assert_eq!(
        dag_repo.get_step(&step_b.id).await.unwrap().get_status(),
        Some(DagStepStatus::Completed)
    );
    assert_eq!(
        job_repo
            .get_jobs_by_pipeline(&created.dag_id)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn startup_recovery_advances_processing_step_from_completed_job_once() {
    let pool = setup_test_pool().await;
    let dag_repo = Arc::new(crate::repositories::dag::SqlxDagRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let job_repo = Arc::new(crate::repositories::job::SqlxJobRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let scheduler = DagScheduler::new(
        Arc::new(JobQueue::with_repository(
            Default::default(),
            job_repo.clone(),
        )),
        dag_repo.clone(),
        job_repo.clone(),
    );
    let created = scheduler
        .create_dag_pipeline(
            two_step_pipeline("recover completed job"),
            &["/input.flv".to_string()],
            DagRunContext::default(),
        )
        .await
        .unwrap();
    let root_job_id = &created.root_job_ids[0];
    sqlx::query("UPDATE job SET status = 'COMPLETED', outputs = ?, completed_at = ? WHERE id = ?")
        .bind(r#"["/durable.mp4"]"#)
        .bind(srec_sqlite::time::now_ms())
        .bind(root_job_id)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 1);
    assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 0);

    let steps = scheduler.get_dag_steps(&created.dag_id).await.unwrap();
    let step_a = steps.iter().find(|step| step.step_id == "A").unwrap();
    let step_b = steps.iter().find(|step| step.step_id == "B").unwrap();
    assert_eq!(step_a.get_status(), Some(DagStepStatus::Completed));
    assert_eq!(step_a.get_outputs(), vec!["/durable.mp4".to_string()]);
    assert_eq!(step_b.get_status(), Some(DagStepStatus::Processing));
    assert!(step_b.job_id.is_some());
    let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
    assert_eq!(dag.completed_steps, 1);
    assert_eq!(dag.failed_steps, 0);
    assert_eq!(
        job_repo
            .get_jobs_by_pipeline(&created.dag_id)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn late_failure_cannot_flip_completed_step_or_dag() {
    let pool = setup_test_pool().await;
    let dag_repo = Arc::new(crate::repositories::dag::SqlxDagRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let job_repo = Arc::new(crate::repositories::job::SqlxJobRepository::new(
        pool.clone(),
        pool,
    ));
    let scheduler = DagScheduler::new(
        Arc::new(JobQueue::with_repository(
            Default::default(),
            job_repo.clone(),
        )),
        dag_repo.clone(),
        job_repo,
    );
    let created = scheduler
        .create_dag_pipeline(
            DagPipelineDefinition::new(
                "terminal failure guard",
                vec![DagStep::new("A", PipelineStep::preset("noop"))],
            ),
            &["/input.flv".to_string()],
            DagRunContext::default(),
        )
        .await
        .unwrap();
    let step = scheduler
        .get_dag_steps(&created.dag_id)
        .await
        .unwrap()
        .remove(0);
    scheduler
        .on_job_completed(
            &step.id,
            &["/output.mp4".to_string()],
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    let update = scheduler
        .on_job_failed(&step.id, "late failure")
        .await
        .unwrap();
    assert_eq!(update.cancelled_count, 0);
    assert!(update.completion.is_none());
    assert_eq!(
        dag_repo.get_step(&step.id).await.unwrap().get_status(),
        Some(DagStepStatus::Completed)
    );
    let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
    assert_eq!(dag.get_status(), Some(DagExecutionStatus::Completed));
    assert_eq!(dag.completed_steps, 1);
    assert_eq!(dag.failed_steps, 0);
}

/// A→{B,C}→D where B fails while C runs. Only D, which depends on B, is
/// cancelled; C keeps its job and finishes; the DAG stays PROCESSING with
/// the error recorded until C settles, and then fails exactly once.
#[tokio::test]
async fn failed_step_cancels_only_its_dependents_and_the_dag_settles_after_siblings() {
    let pool = setup_test_pool().await;
    let dag_repo = Arc::new(crate::repositories::dag::SqlxDagRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let job_repo = Arc::new(crate::repositories::job::SqlxJobRepository::new(
        pool.clone(),
        pool,
    ));
    let scheduler = DagScheduler::new(
        Arc::new(JobQueue::with_repository(
            Default::default(),
            job_repo.clone(),
        )),
        dag_repo.clone(),
        job_repo.clone(),
    );
    let created = scheduler
        .create_dag_pipeline(
            noop_pipeline(
                "diamond",
                &[("A", &[]), ("B", &["A"]), ("C", &["A"]), ("D", &["B", "C"])],
            ),
            &["/input.flv".to_string()],
            DagRunContext::default(),
        )
        .await
        .unwrap();
    let steps = scheduler.get_dag_steps(&created.dag_id).await.unwrap();
    let step_id = |id: &str| {
        steps
            .iter()
            .find(|step| step.step_id == id)
            .unwrap()
            .id
            .clone()
    };

    let update = complete(&scheduler, &step_id("A"), &["/a.mp4"]).await;
    assert_eq!(update.new_job_ids.len(), 2, "B and C fan out");
    assert!(update.completion.is_none());
    assert_eq!(
        scheduler.recover_dag_jobs().await.unwrap(),
        0,
        "D waits on running steps and is not a recovery candidate"
    );

    let update = scheduler
        .on_job_failed(&step_id("B"), "boom")
        .await
        .unwrap();
    assert_eq!(update.cancelled_count, 0, "D had no job to cancel");
    assert!(update.completion.is_none(), "C is still running");
    let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
    assert_eq!(dag.get_status(), Some(DagExecutionStatus::Processing));
    assert_eq!(dag.error.as_deref(), Some("Step 'B' failed: boom"));
    assert_eq!(dag.failed_steps, 1);
    assert!(dag.completed_at.is_none());
    assert_eq!(
        step_statuses(&dag_repo, &created.dag_id).await,
        ["A=COMPLETED", "B=FAILED", "C=PROCESSING", "D=CANCELLED"]
    );
    let step_c = dag_repo.get_step(&step_id("C")).await.unwrap();
    let job_c = job_repo
        .get_job(step_c.job_id.as_deref().unwrap())
        .await
        .unwrap();
    assert_eq!(
        job_c.status, "PENDING",
        "the independent branch keeps its job"
    );

    // A late completion for the cancelled dependent changes nothing.
    let update = complete(&scheduler, &step_id("D"), &["/late.mp4"]).await;
    assert!(update.new_job_ids.is_empty());
    assert!(update.completion.is_none());
    let step_d = dag_repo.get_step(&step_id("D")).await.unwrap();
    assert_eq!(step_d.get_status(), Some(DagStepStatus::Cancelled));
    assert!(step_d.get_outputs().is_empty());
    assert_eq!(
        dag_repo
            .get_dag(&created.dag_id)
            .await
            .unwrap()
            .completed_steps,
        1
    );

    // The last active step finalizes the DAG; the cancelled dependent is
    // not materialized.
    let update = complete(&scheduler, &step_id("C"), &["/c.mp4"]).await;
    assert!(update.new_job_ids.is_empty());
    let completion = update.completion.expect("C settles the DAG");
    assert!(!completion.succeeded);
    let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
    assert_eq!(dag.get_status(), Some(DagExecutionStatus::Failed));
    assert!(dag.completed_at.is_some());
    assert_eq!(dag.completed_steps, 2);
    assert_eq!(dag.failed_steps, 1);
    assert_eq!(dag.error.as_deref(), Some("Step 'B' failed: boom"));
    assert_eq!(
        step_statuses(&dag_repo, &created.dag_id).await,
        ["A=COMPLETED", "B=FAILED", "C=COMPLETED", "D=CANCELLED"]
    );
    assert_eq!(
        job_repo
            .get_jobs_by_pipeline(&created.dag_id)
            .await
            .unwrap()
            .len(),
        3,
        "D never got a job"
    );
}

/// A→B→C where the root fails: nothing else is active, so B and C are
/// cancelled and the DAG fails in the same call. A repeated failure report
/// for the settled step is ignored.
#[tokio::test]
async fn failed_root_cancels_its_chain_and_fails_the_dag_at_once() {
    let pool = setup_test_pool().await;
    let dag_repo = Arc::new(crate::repositories::dag::SqlxDagRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let job_repo = Arc::new(crate::repositories::job::SqlxJobRepository::new(
        pool.clone(),
        pool,
    ));
    let scheduler = DagScheduler::new(
        Arc::new(JobQueue::with_repository(
            Default::default(),
            job_repo.clone(),
        )),
        dag_repo.clone(),
        job_repo.clone(),
    );
    let created = scheduler
        .create_dag_pipeline(
            noop_pipeline("chain", &[("A", &[]), ("B", &["A"]), ("C", &["B"])]),
            &["/input.flv".to_string()],
            DagRunContext::default(),
        )
        .await
        .unwrap();
    let root = scheduler
        .get_dag_steps(&created.dag_id)
        .await
        .unwrap()
        .into_iter()
        .find(|step| step.step_id == "A")
        .unwrap();

    let update = scheduler.on_job_failed(&root.id, "boom").await.unwrap();
    assert_eq!(update.cancelled_count, 0, "the chain had no jobs yet");
    let completion = update.completion.expect("no other step is active");
    assert!(!completion.succeeded);
    let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
    assert_eq!(dag.get_status(), Some(DagExecutionStatus::Failed));
    assert!(dag.completed_at.is_some());
    assert_eq!(dag.completed_steps, 0);
    assert_eq!(dag.failed_steps, 1);
    assert_eq!(dag.error.as_deref(), Some("Step 'A' failed: boom"));
    assert_eq!(
        step_statuses(&dag_repo, &created.dag_id).await,
        ["A=FAILED", "B=CANCELLED", "C=CANCELLED"]
    );
    for step in dag_repo.get_steps_by_dag(&created.dag_id).await.unwrap() {
        if step.step_id != "A" {
            assert!(step.job_id.is_none(), "{} never got a job", step.step_id);
        }
    }

    let update = scheduler.on_job_failed(&root.id, "again").await.unwrap();
    assert_eq!(update.cancelled_count, 0);
    assert!(update.completion.is_none());
    assert_eq!(
        dag_repo
            .get_dag(&created.dag_id)
            .await
            .unwrap()
            .failed_steps,
        1
    );
}

/// The plan follows the step's policy, doubling the wait per retry and
/// stopping when the budget is spent, and the budget is reported for the
/// job page; the step's timeout reaches its job.
#[tokio::test]
async fn plan_step_retry_follows_the_step_policy_and_stops_when_spent() {
    use srec_sqlite::models::StepRetryPolicy;

    let pool = setup_test_pool().await;
    let dag_repo = Arc::new(crate::repositories::dag::SqlxDagRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let job_repo = Arc::new(crate::repositories::job::SqlxJobRepository::new(
        pool.clone(),
        pool,
    ));
    let scheduler = DagScheduler::new(
        Arc::new(JobQueue::with_repository(
            Default::default(),
            job_repo.clone(),
        )),
        dag_repo.clone(),
        job_repo.clone(),
    );
    let created = scheduler
        .create_dag_pipeline(
            DagPipelineDefinition::new(
                "retry policy",
                vec![
                    DagStep::new("A", PipelineStep::inline("noop", serde_json::json!({})))
                        .with_retry(StepRetryPolicy {
                            max_attempts: 3,
                            backoff_secs: 10,
                        })
                        .with_timeout_secs(600),
                    DagStep::new("B", PipelineStep::inline("noop", serde_json::json!({}))),
                ],
            ),
            &["/input.flv".to_string()],
            DagRunContext::default(),
        )
        .await
        .unwrap();
    let steps = scheduler.get_dag_steps(&created.dag_id).await.unwrap();
    let step_id = |id: &str| {
        steps
            .iter()
            .find(|step| step.step_id == id)
            .unwrap()
            .id
            .clone()
    };
    let job_of = |id: &str| {
        steps
            .iter()
            .find(|step| step.step_id == id)
            .unwrap()
            .job_id
            .clone()
            .unwrap()
    };

    let before = Timestamp::now();
    let first = scheduler
        .plan_step_retry(&step_id("A"), 0)
        .await
        .unwrap()
        .expect("first retry");
    assert_eq!((first.next_attempt, first.max_attempts), (2, 3));
    let wait = first.retry_after.duration_since(before);
    assert!(
        wait >= jiff::SignedDuration::from_secs(10) && wait < jiff::SignedDuration::from_secs(12),
        "{wait}"
    );
    let second = scheduler
        .plan_step_retry(&step_id("A"), 1)
        .await
        .unwrap()
        .expect("second retry");
    assert_eq!(second.next_attempt, 3);
    assert!(second.retry_after.duration_since(before) >= jiff::SignedDuration::from_secs(20));
    assert!(
        scheduler
            .plan_step_retry(&step_id("A"), 2)
            .await
            .unwrap()
            .is_none(),
        "budget spent"
    );
    assert!(
        scheduler
            .plan_step_retry(&step_id("B"), 0)
            .await
            .unwrap()
            .is_none(),
        "no policy"
    );
    assert_eq!(
        scheduler
            .step_retry_state(&step_id("A"), &job_of("A"))
            .await
            .unwrap(),
        StepRetryState {
            max_attempts: Some(3),
            awaits_job: true
        }
    );
    assert_eq!(
        scheduler
            .step_retry_state(&step_id("B"), &job_of("B"))
            .await
            .unwrap()
            .max_attempts,
        None
    );

    let job_a = job_repo.get_job(&job_of("A")).await.unwrap();
    assert_eq!(
        crate::job_queue::parse_job_state(&job_a.state).timeout_secs,
        Some(600)
    );
    let job_b = job_repo.get_job(&job_of("B")).await.unwrap();
    assert_eq!(
        crate::job_queue::parse_job_state(&job_b.state).timeout_secs,
        None
    );

    assert!(
        scheduler
            .step_awaits_job(&step_id("A"), &job_of("A"))
            .await
            .unwrap()
    );
    assert!(
        !scheduler
            .step_awaits_job(&step_id("A"), "another-job")
            .await
            .unwrap()
    );
    assert!(
        !scheduler
            .step_awaits_job("missing-step", &job_of("A"))
            .await
            .unwrap()
    );

    // A step that failed for good plans nothing and awaits nothing.
    scheduler
        .on_job_failed(&step_id("A"), "boom")
        .await
        .unwrap();
    assert!(
        scheduler
            .plan_step_retry(&step_id("A"), 0)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !scheduler
            .step_awaits_job(&step_id("A"), &job_of("A"))
            .await
            .unwrap()
    );
}

/// The worker's failure entry point. A retryable failure with budget left
/// is recorded on the job row together with its retry time and leaves the
/// step PROCESSING; once the budget is spent it reaches the workflow. A
/// kind that cannot succeed again and a job row that could not be written
/// fail the step instead; a report for a settled step is ignored.
#[tokio::test]
async fn job_attempt_failure_is_retried_or_applied_by_kind_budget_and_persistence() {
    use crate::repositories::JobRepository as _;
    use srec_sqlite::models::StepRetryPolicy;

    let pool = setup_test_pool().await;
    let dag_repo = Arc::new(crate::repositories::dag::SqlxDagRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let job_repo = Arc::new(crate::repositories::job::SqlxJobRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let queue = Arc::new(JobQueue::with_repository(
        Default::default(),
        job_repo.clone(),
    ));
    let scheduler = DagScheduler::new(queue.clone(), dag_repo.clone(), job_repo.clone());
    let retrying = |id: &str| {
        DagStep::new(id, PipelineStep::inline("noop", serde_json::json!({}))).with_retry(
            StepRetryPolicy {
                max_attempts: 2,
                backoff_secs: 30,
            },
        )
    };
    let created = scheduler
        .create_dag_pipeline(
            DagPipelineDefinition::new(
                "attempt failures",
                vec![
                    retrying("A"),
                    retrying("B"),
                    retrying("E"),
                    retrying("F"),
                    DagStep::with_dependencies(
                        "D",
                        PipelineStep::inline("noop", serde_json::json!({})),
                        vec!["A".to_string()],
                    ),
                ],
            ),
            &["/input.flv".to_string()],
            DagRunContext::default(),
        )
        .await
        .unwrap();
    let steps = scheduler.get_dag_steps(&created.dag_id).await.unwrap();
    let step_id = |id: &str| {
        steps
            .iter()
            .find(|step| step.step_id == id)
            .unwrap()
            .id
            .clone()
    };
    let job_of = |id: &str| {
        steps
            .iter()
            .find(|step| step.step_id == id)
            .unwrap()
            .job_id
            .clone()
            .unwrap()
    };
    let record = |job_id: String| {
        let queue = queue.clone();
        let job_repo = job_repo.clone();
        move |planned: Option<PlannedRetry>| async move {
            let retry_after = planned.map(|planned| planned.retry_after);
            let recorded = queue
                .fail_with_step_info(&job_id, "boom", retry_after, Some("noop"), &[])
                .await;
            // The failure write itself carries the planned retry, so a
            // reader or a restart right after it never sees one without
            // the other.
            if matches!(recorded, Ok(JobFailureOutcome::Transitioned)) {
                assert_eq!(
                    job_repo.get_job(&job_id).await.unwrap().retry_after,
                    retry_after.map(|retry_after| retry_after.as_millisecond())
                );
            }
            recorded
        }
    };
    fn attempt(job_id: &str, retry_count: i32, kind: StepFailureKind) -> FailedStepAttempt<'_> {
        FailedStepAttempt {
            job_id,
            retry_count,
            error: "boom",
            kind,
        }
    }

    // A: budget left, so the attempt is recorded and a retry scheduled.
    let job_a = job_of("A");
    let outcome = scheduler
        .on_job_attempt_failed(
            &step_id("A"),
            attempt(&job_a, 0, StepFailureKind::ProcessorError),
            record(job_a.clone()),
        )
        .await
        .unwrap();
    let StepFailureOutcome::Retried(planned) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!((planned.next_attempt, planned.max_attempts), (2, 2));
    let row = job_repo.get_job(&job_a).await.unwrap();
    assert_eq!(row.status, JobStatus::Failed.as_str());
    assert_eq!(row.retry_after, Some(planned.retry_after.as_millisecond()));
    assert_eq!(
        step_statuses(&dag_repo, &created.dag_id).await,
        [
            "A=PROCESSING",
            "B=PROCESSING",
            "D=BLOCKED",
            "E=PROCESSING",
            "F=PROCESSING"
        ]
    );
    assert!(
        dag_repo
            .get_dag(&created.dag_id)
            .await
            .unwrap()
            .error
            .is_none()
    );

    // The sweeper re-queues A; its next failure spends the budget and
    // reaches the workflow: D is cancelled while the siblings keep running.
    queue.retry_job(&job_a).await.unwrap();
    let outcome = scheduler
        .on_job_attempt_failed(
            &step_id("A"),
            attempt(&job_a, 1, StepFailureKind::ProcessorError),
            record(job_a.clone()),
        )
        .await
        .unwrap();
    let StepFailureOutcome::Failed(update) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(update.cancelled_count, 0, "D had no job");
    assert!(update.completion.is_none(), "siblings still run");
    let row = job_repo.get_job(&job_a).await.unwrap();
    assert!(row.retry_after.is_none());
    let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
    assert_eq!(dag.get_status(), Some(DagExecutionStatus::Processing));
    assert_eq!(dag.error.as_deref(), Some("Step 'A' failed: boom"));

    // B: the kind rules out another attempt, budget or not.
    let job_b = job_of("B");
    let outcome = scheduler
        .on_job_attempt_failed(
            &step_id("B"),
            attempt(&job_b, 0, StepFailureKind::NonRetryable),
            record(job_b.clone()),
        )
        .await
        .unwrap();
    assert!(
        matches!(outcome, StepFailureOutcome::Failed(_)),
        "{outcome:?}"
    );
    let row = job_repo.get_job(&job_b).await.unwrap();
    assert!(row.retry_after.is_none());
    // A repeated report for the settled step changes nothing.
    let outcome = scheduler
        .on_job_attempt_failed(
            &step_id("B"),
            attempt(&job_b, 0, StepFailureKind::ProcessorError),
            record(job_b.clone()),
        )
        .await
        .unwrap();
    assert!(
        matches!(outcome, StepFailureOutcome::Ignored),
        "{outcome:?}"
    );

    // E: an unrecorded failure cannot settle the step. Its owner can retry persistence.
    let job_e = job_of("E");
    let error = scheduler
        .on_job_attempt_failed(
            &step_id("E"),
            attempt(&job_e, 0, StepFailureKind::ProcessorError),
            |_planned| async { Err(Error::database("disk full")) },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Core(CoreError::Database(_))));
    assert_eq!(
        dag_repo.get_step(&step_id("E")).await.unwrap().get_status(),
        Some(DagStepStatus::Processing)
    );
    assert_eq!(
        job_repo.get_job(&job_e).await.unwrap().get_status(),
        Some(JobStatus::Pending)
    );
    let outcome = scheduler
        .on_job_attempt_failed(
            &step_id("E"),
            attempt(&job_e, 0, StepFailureKind::ExecutionStart),
            record(job_e.clone()),
        )
        .await
        .unwrap();
    assert!(matches!(outcome, StepFailureOutcome::Failed(_)));

    // F: the processor never ran; the last active step settles the DAG.
    let job_f = job_of("F");
    let outcome = scheduler
        .on_job_attempt_failed(
            &step_id("F"),
            attempt(&job_f, 0, StepFailureKind::ExecutionStart),
            record(job_f.clone()),
        )
        .await
        .unwrap();
    let StepFailureOutcome::Failed(update) = outcome else {
        panic!("{outcome:?}");
    };
    let completion = update.completion.expect("no step is left active");
    assert!(!completion.succeeded);
    assert_eq!(
        step_statuses(&dag_repo, &created.dag_id).await,
        [
            "A=FAILED",
            "B=FAILED",
            "D=CANCELLED",
            "E=FAILED",
            "F=FAILED"
        ]
    );
    let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
    assert_eq!(dag.get_status(), Some(DagExecutionStatus::Failed));
    assert_eq!(dag.error.as_deref(), Some("Step 'A' failed: boom"));
    assert_eq!(dag.failed_steps, 4);
}

#[tokio::test]
async fn recovery_settles_active_jobs_of_terminal_workflows_before_claiming() {
    use crate::repositories::{SqlxDagRepository, SqlxJobRepository};

    for initial_job_status in ["PENDING", "PROCESSING"] {
        for (dag_status, step_status, expected) in [
            ("FAILED", "FAILED", JobStatus::Failed),
            ("FAILED", "PROCESSING", JobStatus::Failed),
            ("CANCELLED", "CANCELLED", JobStatus::Cancelled),
            ("COMPLETED", "COMPLETED", JobStatus::Cancelled),
            ("PROCESSING", "FAILED", JobStatus::Failed),
            ("PROCESSING", "CANCELLED", JobStatus::Cancelled),
        ] {
            let pool = srec_sqlite::init_pool_with_size("sqlite::memory:", 1)
                .await
                .unwrap();
            srec_sqlite::run_migrations(&pool).await.unwrap();
            let jobs = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
            let dags = Arc::new(SqlxDagRepository::new(pool.clone(), pool.clone()));
            let queue = Arc::new(JobQueue::with_repository(Default::default(), jobs.clone()));
            let scheduler = DagScheduler::new(queue.clone(), dags.clone(), jobs.clone());
            let created = scheduler
                .create_dag_pipeline(
                    DagPipelineDefinition::new(
                        "stale active job",
                        vec![DagStep::new(
                            "A",
                            PipelineStep::inline("remux", serde_json::json!({})),
                        )],
                    ),
                    &["input.flv".into()],
                    DagRunContext::default(),
                )
                .await
                .unwrap();
            let (job, _) = queue.dequeue(None).await.unwrap().unwrap();
            let step_id = job.dag_step_execution_id.as_deref().unwrap();
            // Model rows left by the old failure path or an interrupted cancellation.
            sqlx::query("UPDATE job SET status = ? WHERE id = ?")
                .bind(initial_job_status)
                .bind(&job.id)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("UPDATE dag_step_execution SET status = ? WHERE id = ?")
                .bind(step_status)
                .bind(step_id)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("UPDATE dag_execution SET status = ? WHERE id = ?")
                .bind(dag_status)
                .bind(&created.dag_id)
                .execute(&pool)
                .await
                .unwrap();
            assert!(jobs.claim_next_pending_job(None).await.unwrap().is_none());
            assert!(
                jobs.claim_next_pending_job(Some(&["remux".to_owned()]))
                    .await
                    .unwrap()
                    .is_none()
            );
            scheduler.recover_dag_jobs().await.unwrap();
            assert_eq!(queue.recover_jobs().await.unwrap(), 0);
            let stored = jobs.get_job(&job.id).await.unwrap();
            assert_eq!(stored.get_status(), Some(expected));
            assert!(stored.completed_at.is_some());
            assert!(stored.retry_after.is_none());
            assert_eq!(queue.depth(), 0);
            assert!(queue.get_cancellation_token(&job.id).await.is_none());
            assert!(queue.dequeue(None).await.unwrap().is_none());
            assert_eq!(
                queue.recover_jobs().await.unwrap(),
                0,
                "recovery is idempotent"
            );
            if dag_status == "FAILED" && step_status == "FAILED" {
                // Repair also makes an explicit workflow retry possible again.
                scheduler.retry_dag(&created.dag_id, || {}).await.unwrap();
                assert_eq!(queue.dequeue(None).await.unwrap().unwrap().0.id, job.id);
            }
            queue.stop_progress_aggregator().await;
        }
    }
}

#[tokio::test]
async fn cancelled_attempt_does_not_schedule_retry_or_apply_step_failure() {
    use crate::repositories::{SqlxDagRepository, SqlxJobRepository};
    use srec_sqlite::models::StepRetryPolicy;
    use srec_sqlite::test_support::SqlTrace;

    let pool = srec_sqlite::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    srec_sqlite::run_migrations(&pool).await.unwrap();
    let dags = Arc::new(SqlxDagRepository::new(pool.clone(), pool.clone()));
    let jobs = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
    let queue = Arc::new(JobQueue::with_repository(Default::default(), jobs.clone()));
    let scheduler = DagScheduler::new(queue.clone(), dags.clone(), jobs.clone());
    let created = scheduler
        .create_dag_pipeline(
            DagPipelineDefinition::new(
                "cancel race",
                vec![
                    DagStep::new("A", PipelineStep::inline("noop", serde_json::json!({})))
                        .with_retry(StepRetryPolicy {
                            max_attempts: 2,
                            backoff_secs: 1,
                        }),
                ],
            ),
            &["input.flv".into()],
            DagRunContext::default(),
        )
        .await
        .unwrap();
    let (job, _) = queue.dequeue(None).await.unwrap().unwrap();
    let trace = std::sync::Mutex::new(None);
    let outcome = scheduler
        .on_job_attempt_failed(
            job.dag_step_execution_id.as_deref().unwrap(),
            FailedStepAttempt {
                job_id: &job.id,
                retry_count: 0,
                error: "late failure",
                kind: StepFailureKind::ProcessorError,
            },
            |planned| {
                let scheduler = &scheduler;
                let queue = &queue;
                let pool = &pool;
                let trace = &trace;
                let job_id = &job.id;
                let dag_id = &created.dag_id;
                async move {
                    // Cancellation wins after retry planning but before the conditional job update.
                    scheduler.cancel_dag(dag_id).await.unwrap();
                    let observer = SqlTrace::install(pool).await;
                    *trace.lock().unwrap() = Some(observer);
                    queue
                        .fail_with_step_info(
                            job_id,
                            "late failure",
                            planned.map(|planned| planned.retry_after),
                            Some("noop"),
                            &[],
                        )
                        .await
                }
            },
        )
        .await
        .unwrap();
    assert!(matches!(outcome, StepFailureOutcome::Ignored));
    let statements = trace.lock().unwrap().take().unwrap().statements();
    assert!(
        !statements
            .iter()
            .any(|sql| sql.contains("UPDATE dag_step_execution")),
        "{statements:?}"
    );
    let stored = jobs.get_job(&job.id).await.unwrap();
    assert_eq!(stored.get_status(), Some(JobStatus::Cancelled));
    assert!(stored.retry_after.is_none());
    assert_eq!(
        dags.get_dag(&created.dag_id).await.unwrap().get_status(),
        Some(DagExecutionStatus::Cancelled)
    );
    assert_eq!(queue.depth(), 0);
    assert!(queue.get_cancellation_token(&job.id).await.is_none());
    queue.stop_progress_aggregator().await;
}

#[tokio::test]
async fn failed_attempt_observability_errors_preserve_retry_and_cleanup() {
    use crate::repositories::JobRepository as _;
    use srec_sqlite::models::StepRetryPolicy;

    for fault in [
        "CREATE TRIGGER fail_observability BEFORE INSERT ON job_execution_logs
         BEGIN SELECT RAISE(ABORT, 'log write failed'); END",
        "CREATE TRIGGER fail_observability BEFORE UPDATE OF execution_info ON job
         BEGIN SELECT RAISE(ABORT, 'summary write failed'); END",
    ] {
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let queue = Arc::new(JobQueue::with_repository(
            Default::default(),
            job_repo.clone(),
        ));
        let scheduler = DagScheduler::new(queue.clone(), dag_repo.clone(), job_repo.clone());
        let created = scheduler
            .create_dag_pipeline(
                DagPipelineDefinition::new(
                    "retry despite observability failure",
                    vec![
                        DagStep::new(
                            "upload",
                            PipelineStep::inline("rclone", serde_json::json!({})),
                        )
                        .with_retry(StepRetryPolicy {
                            max_attempts: 2,
                            backoff_secs: 30,
                        }),
                    ],
                ),
                &["/input.flv".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        let (job, _) = queue.dequeue(None).await.unwrap().unwrap();
        assert_eq!(queue.depth(), 1);
        assert!(queue.get_cancellation_token(&job.id).await.is_some());
        sqlx::query(fault).execute(&pool).await.unwrap();

        let outcome = scheduler
            .on_job_attempt_failed(
                job.dag_step_execution_id.as_deref().unwrap(),
                FailedStepAttempt {
                    job_id: &job.id,
                    retry_count: 0,
                    error: "upload failed",
                    kind: StepFailureKind::ProcessorError,
                },
                |planned| {
                    let queue = &queue;
                    let job_id = &job.id;
                    async move {
                        queue
                            .fail_with_step_info(
                                job_id,
                                "upload failed",
                                planned.map(|planned| planned.retry_after),
                                Some("rclone"),
                                &[],
                            )
                            .await
                    }
                },
            )
            .await
            .unwrap();
        let StepFailureOutcome::Retried(planned) = outcome else {
            panic!("observability failure suppressed retry: {outcome:?}");
        };
        let stored = job_repo.get_job(&job.id).await.unwrap();
        assert_eq!(stored.get_status(), Some(JobStatus::Failed));
        assert_eq!(
            stored.retry_after,
            Some(planned.retry_after.as_millisecond())
        );
        assert_eq!(queue.depth(), 0);
        assert!(queue.get_cancellation_token(&job.id).await.is_none());
        assert_eq!(
            step_statuses(&dag_repo, &created.dag_id).await,
            ["upload=PROCESSING"]
        );
        assert_eq!(
            dag_repo
                .get_dag(&created.dag_id)
                .await
                .unwrap()
                .get_status(),
            Some(DagExecutionStatus::Processing)
        );
        queue.stop_progress_aggregator().await;
    }
}

/// The worker's completion entry point fails a workflow it cannot advance
/// from the result, so the DAG settles and the worker still receives a
/// completion to forward.
#[tokio::test]
async fn job_attempt_completion_fails_the_dag_it_cannot_advance() {
    let pool = setup_test_pool().await;
    let dag_repo = Arc::new(crate::repositories::dag::SqlxDagRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let job_repo = Arc::new(crate::repositories::job::SqlxJobRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let scheduler = DagScheduler::new(
        Arc::new(JobQueue::with_repository(
            Default::default(),
            job_repo.clone(),
        )),
        dag_repo.clone(),
        job_repo.clone(),
    );
    let created = scheduler
        .create_dag_pipeline(
            two_step_pipeline("unreadable definition"),
            &["/input.flv".to_string()],
            DagRunContext::default(),
        )
        .await
        .unwrap();
    let steps = scheduler.get_dag_steps(&created.dag_id).await.unwrap();
    let step_a = steps.iter().find(|step| step.step_id == "A").unwrap();
    // Fan-out to B needs the definition; make it unreadable.
    sqlx::query("UPDATE dag_execution SET dag_definition = '{' WHERE id = ?")
        .bind(&created.dag_id)
        .execute(&pool)
        .await
        .unwrap();

    let update = scheduler
        .on_job_attempt_completed(&step_a.id, &["/a.mp4".to_string()], None, None, None, None)
        .await
        .unwrap();
    assert!(update.new_job_ids.is_empty());
    let completion = update.completion.expect("the failure settles the DAG");
    assert!(!completion.succeeded);
    assert_eq!(completion.dag_id, created.dag_id);
    let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
    assert_eq!(dag.get_status(), Some(DagExecutionStatus::Failed));
    assert!(
        dag.error
            .as_deref()
            .unwrap()
            .starts_with("DAG scheduler error: "),
        "{:?}",
        dag.error
    );
    assert_eq!(
        step_statuses(&dag_repo, &created.dag_id).await,
        ["A=COMPLETED", "B=CANCELLED"],
        "the step's own completion stands; the dependent it could not reach is cancelled"
    );
}

/// `retry_dag` runs `after_reset` once the rows are reset and before any
/// job is restarted; a rejected retry never runs it and leaves the rows
/// alone; a retry that breaks down after the reset has run it, re-fails
/// the DAG and hands back that failure's completion.
#[tokio::test]
async fn retry_dag_runs_the_reset_hook_between_reset_and_restart() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let pool = setup_test_pool().await;
    let dag_repo = Arc::new(crate::repositories::dag::SqlxDagRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let job_repo = Arc::new(crate::repositories::job::SqlxJobRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let queue = Arc::new(JobQueue::with_repository(
        Default::default(),
        job_repo.clone(),
    ));
    let scheduler = DagScheduler::new(queue.clone(), dag_repo.clone(), job_repo.clone());
    let failed_chain = |name: &str| {
        let scheduler = &scheduler;
        let job_repo = &job_repo;
        let name = name.to_string();
        async move {
            let created = scheduler
                .create_dag_pipeline(
                    noop_pipeline(&name, &[("A", &[]), ("B", &["A"])]),
                    &["/input.flv".to_string()],
                    DagRunContext::default(),
                )
                .await
                .unwrap();
            let root = scheduler
                .get_dag_steps(&created.dag_id)
                .await
                .unwrap()
                .into_iter()
                .find(|step| step.step_id == "A")
                .unwrap();
            crate::repositories::JobRepository::mark_job_failed(
                job_repo.as_ref(),
                &created.root_job_ids[0],
                "boom",
                None,
            )
            .await
            .unwrap();
            let update = scheduler.on_job_failed(&root.id, "boom").await.unwrap();
            assert!(update.completion.is_some());
            created
        }
    };

    // Rejected before any row changes: the DAG is still running.
    let running = scheduler
        .create_dag_pipeline(
            noop_pipeline("running", &[("A", &[])]),
            &["/input.flv".to_string()],
            DagRunContext::default(),
        )
        .await
        .unwrap();
    let hook_calls = AtomicUsize::new(0);
    let error = scheduler
        .retry_dag(&running.dag_id, || {
            hook_calls.fetch_add(1, Ordering::SeqCst);
        })
        .await
        .unwrap_err();
    assert!(
        matches!(&error, DagRetryError::Rejected(Error::Core(CoreError::Validation(message))) if message.contains("not in FAILED or CANCELLED")),
        "{error:?}"
    );
    assert_eq!(hook_calls.load(Ordering::SeqCst), 0);

    // Retried: the hook sees the reset rows before the restart re-queues A.
    let created = failed_chain("retried").await;
    let depth_before = queue.depth();
    let depth_at_hook = AtomicUsize::new(usize::MAX);
    let update = scheduler
        .retry_dag(&created.dag_id, || {
            hook_calls.fetch_add(1, Ordering::SeqCst);
            depth_at_hook.store(queue.depth(), Ordering::SeqCst);
        })
        .await
        .unwrap();
    assert_eq!(hook_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        depth_at_hook.load(Ordering::SeqCst),
        depth_before,
        "nothing re-queued yet"
    );
    assert_eq!(
        queue.depth(),
        depth_before + 1,
        "A was re-queued after the hook"
    );
    assert_eq!(
        (
            update.retryable_steps,
            update.retried_steps,
            update.reconciled_steps
        ),
        (2, 2, 0)
    );
    assert_eq!(update.job_ids, vec![created.root_job_ids[0].clone()]);
    assert_eq!(update.restarted_jobs.len(), 1);
    assert_eq!(update.restarted_jobs[0].id, created.root_job_ids[0]);
    assert!(update.completions.is_empty());
    assert_eq!(
        step_statuses(&dag_repo, &created.dag_id).await,
        ["A=PROCESSING", "B=BLOCKED"]
    );
    assert_eq!(
        job_repo
            .get_job(&created.root_job_ids[0])
            .await
            .unwrap()
            .status,
        JobStatus::Pending.as_str()
    );

    // Refailed: the reset took effect, then fan-out could not read the definition.
    let created = failed_chain("refailed").await;
    sqlx::query("UPDATE dag_execution SET dag_definition = '{' WHERE id = ?")
        .bind(&created.dag_id)
        .execute(&pool)
        .await
        .unwrap();
    let error = scheduler
        .retry_dag(&created.dag_id, || {
            hook_calls.fetch_add(1, Ordering::SeqCst);
        })
        .await
        .unwrap_err();
    assert_eq!(hook_calls.load(Ordering::SeqCst), 2);
    let DagRetryError::Refailed { error, completion } = error else {
        panic!("{error:?}");
    };
    assert!(error.to_string().contains("DAG definition"), "{error}");
    let completion = completion.expect("the re-failed DAG settles");
    assert!(!completion.succeeded);
    assert_eq!(completion.dag_id, created.dag_id);
    let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
    assert_eq!(dag.get_status(), Some(DagExecutionStatus::Failed));
    assert!(
        dag.error.as_deref().unwrap().starts_with("Retry failed:"),
        "{:?}",
        dag.error
    );
    assert_eq!(
        step_statuses(&dag_repo, &created.dag_id).await,
        ["A=CANCELLED", "B=CANCELLED"],
        "no step is left PROCESSING"
    );
    assert_eq!(
        job_repo
            .get_job(&created.root_job_ids[0])
            .await
            .unwrap()
            .status,
        JobStatus::Failed.as_str(),
        "the job was never restarted"
    );
}

#[tokio::test]
async fn dag_publication_rolls_back_all_rows_when_root_attachment_fails() {
    let pool = setup_test_pool().await;
    let dag_repo = crate::repositories::dag::SqlxDagRepository::new(pool.clone(), pool.clone());
    let definition = DagPipelineDefinition::new(
        "atomic publication",
        vec![DagStep::new("A", PipelineStep::preset("noop"))],
    );
    let mut dag = DagExecutionDbModel::new(&definition, None, None);
    dag.status = DagExecutionStatus::Processing.as_str().to_string();
    let mut step = DagStepExecutionDbModel::new(&dag.id, "A", &[]);
    let mut job = JobDbModel::new_pipeline_step("noop", "[]", "[]", 0, None, None);
    step.status = DagStepStatus::Processing.as_str().to_string();
    step.job_id = Some(job.id.clone());
    job.pipeline_id = Some(dag.id.clone());
    job.dag_step_execution_id = Some("missing-step".to_string());

    assert!(
        dag_repo
            .publish_dag(&dag, &[step], &[job.clone()])
            .await
            .is_err()
    );
    assert!(dag_repo.get_dag(&dag.id).await.is_err());
    assert!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM dag_step_execution WHERE dag_id = ?")
            .bind(&dag.id)
            .fetch_one(&pool)
            .await
            .unwrap()
            == 0
    );
    assert!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM job WHERE id = ?")
            .bind(&job.id)
            .fetch_one(&pool)
            .await
            .unwrap()
            == 0
    );
}

/// Cancelling a DAG counts the running steps it stopped: a step whose job
/// waits for its automatic retry counts, although that FAILED job cannot
/// be cancelled itself, and a step that has no job yet does not.
#[tokio::test]
async fn test_cancel_dag_marks_parent_cancelled() {
    let pool = setup_test_pool().await;
    let dag_repo = Arc::new(crate::repositories::dag::SqlxDagRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let queue = Arc::new(JobQueue::with_repository(
        Default::default(),
        Arc::new(crate::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool,
        )),
    ));
    let scheduler = DagScheduler::new(queue.clone(), dag_repo.clone(), Arc::new(NoopJobRepository));

    let created = scheduler
        .create_dag_pipeline(
            DagPipelineDefinition::new(
                "test",
                vec![
                    DagStep::new("A", PipelineStep::inline("noop", serde_json::json!({}))),
                    DagStep::with_dependencies(
                        "B",
                        PipelineStep::inline("noop", serde_json::json!({})),
                        vec!["A".to_string()],
                    ),
                ],
            ),
            &["/input.flv".to_string()],
            DagRunContext::default(),
        )
        .await
        .unwrap();
    // A's job failed and waits for its automatic retry.
    queue
        .fail_with_step_info(
            &created.root_job_ids[0],
            "boom",
            Some(Timestamp::now() + jiff::SignedDuration::from_secs(30)),
            Some("noop"),
            &[],
        )
        .await
        .unwrap();

    let result = scheduler
        .cancel_dag_with_completion(&created.dag_id)
        .await
        .unwrap();
    assert_eq!(result.cancelled_count, 1);
    assert_eq!(
        step_statuses(&dag_repo, &created.dag_id).await,
        ["A=CANCELLED", "B=CANCELLED"]
    );

    let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
    assert_eq!(
        dag.status,
        srec_sqlite::models::DagExecutionStatus::Cancelled.as_str()
    );
}
