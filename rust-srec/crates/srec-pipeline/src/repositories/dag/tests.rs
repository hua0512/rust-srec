use std::collections::HashMap;

use sqlx::SqlitePool;

use super::*;
use crate::repositories::{JobRepository as _, SqlxJobRepository};
use srec_sqlite::models::{
    DagDisplayCounts, DagExecutionDbModel, DagExecutionListSummary, DagExecutionStatus,
    DagPipelineDefinition, DagStep, DagStepExecutionDbModel, DagStepStatus, JobDbModel,
    PipelineStep,
};
use srec_sqlite::{init_pool_with_size, run_migrations};

async fn setup_test_pool() -> SqlitePool {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    pool
}

async fn create_job(pool: &SqlitePool, id: &str) {
    let mut job = JobDbModel::new("test-job", "{}");
    job.id = id.to_string();
    SqlxJobRepository::new(pool.clone(), pool.clone())
        .create_job(&job)
        .await
        .unwrap();
}

/// The manifest is published in the same transaction as the steps and root
/// jobs and comes back verbatim, so a restart or retry never has to look for
/// pairing information outside the DAG row.
#[tokio::test]
async fn publish_dag_round_trips_the_input_manifest() {
    let pool = setup_test_pool().await;
    let repo = SqlxDagRepository::new(pool.clone(), pool.clone());
    let dag_def = DagPipelineDefinition::new(
        "manifest-dag",
        vec![DagStep::new("A", PipelineStep::preset("remux"))],
    );
    let mut dag = DagExecutionDbModel::new(&dag_def, None, Some("session".to_string()));
    dag.status = DagExecutionStatus::Processing.as_str().to_string();
    let manifest = serde_json::json!({
        "version": 1,
        "session_id": "session",
        "streamer_id": "streamer",
        "scope": { "segment": { "index": 3 } },
        "segments": [{ "segment_index": 3, "video": ["/rec/3.mp4"], "danmu": ["/rec/3.xml"] }],
    })
    .to_string();
    dag.input_manifest = Some(manifest.clone());
    let mut step = DagStepExecutionDbModel::new(&dag.id, "A", &[]);
    let mut job = JobDbModel::new_pipeline_step("remux", "[]", "[]", 0, None, None);
    step.status = DagStepStatus::Processing.as_str().to_string();
    step.job_id = Some(job.id.clone());
    job.pipeline_id = Some(dag.id.clone());
    job.dag_step_execution_id = Some(step.id.clone());

    repo.publish_dag(&dag, &[step], &[job]).await.unwrap();

    let stored = repo.get_dag(&dag.id).await.unwrap();
    assert_eq!(stored.input_manifest.as_deref(), Some(manifest.as_str()));
    let listed = repo.list_dags(None, Some("session"), 10, 0).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].input_manifest, stored.input_manifest);
    assert_eq!(
        repo.count_dags(Some(DagExecutionStatus::Processing), Some("session"))
            .await
            .unwrap(),
        1
    );
    assert!(
        repo.list_dags(Some(DagExecutionStatus::Completed), Some("session"), 10, 0)
            .await
            .unwrap()
            .is_empty()
    );

    let mut plain = DagExecutionDbModel::new(&dag_def, None, None);
    plain.status = DagExecutionStatus::Processing.as_str().to_string();
    repo.create_dag(&plain).await.unwrap();
    assert_eq!(repo.get_dag(&plain.id).await.unwrap().input_manifest, None);
}

/// An unfinished DAG lists, filters, and counts as PENDING until one of its
/// jobs is running, even though its stored status is PROCESSING.
#[tokio::test]
async fn display_status_reports_unfinished_dags_without_a_running_job_as_pending() {
    let pool = setup_test_pool().await;
    let repo = SqlxDagRepository::new(pool.clone(), pool.clone());
    let jobs = SqlxJobRepository::new(pool.clone(), pool.clone());
    let dag_def = DagPipelineDefinition::new(
        "display",
        vec![DagStep::new("A", PipelineStep::preset("remux"))],
    );

    let mut queued = DagExecutionDbModel::new(&dag_def, None, None);
    queued.status = DagExecutionStatus::Processing.as_str().to_string();
    queued.created_at = 1_000;
    let mut running = DagExecutionDbModel::new(&dag_def, None, None);
    running.status = DagExecutionStatus::Processing.as_str().to_string();
    running.created_at = 2_000;
    let other_def = DagPipelineDefinition::new(
        "Upload_Backup",
        vec![DagStep::new("A", PipelineStep::preset("remux"))],
    );
    let mut completed = DagExecutionDbModel::new(&other_def, None, None);
    completed.status = DagExecutionStatus::Completed.as_str().to_string();
    completed.created_at = 3_000;
    completed.completed_at = Some(7_000);
    for dag in [&queued, &running, &completed] {
        repo.create_dag(dag).await.unwrap();
    }
    for (dag, status) in [(&queued, "PENDING"), (&running, "PROCESSING")] {
        let mut job = JobDbModel::new_pipeline_step("remux", "[]", "[]", 0, None, None);
        job.pipeline_id = Some(dag.id.clone());
        job.status = status.to_string();
        jobs.create_job(&job).await.unwrap();
    }

    let listed = repo
        .list_dag_summaries(&DagListFilter::default(), 10, 0)
        .await
        .unwrap();
    let listed: Vec<_> = listed
        .iter()
        .map(|dag| (dag.id.as_str(), dag.display_status))
        .collect();
    assert_eq!(
        listed,
        vec![
            (completed.id.as_str(), DagExecutionStatus::Completed),
            (running.id.as_str(), DagExecutionStatus::Processing),
            (queued.id.as_str(), DagExecutionStatus::Pending),
        ]
    );

    for (status, expected) in [
        (DagExecutionStatus::Pending, &queued),
        (DagExecutionStatus::Processing, &running),
        (DagExecutionStatus::Completed, &completed),
    ] {
        let filter = DagListFilter {
            status: Some(status),
            ..Default::default()
        };
        let filtered = repo.list_dag_summaries(&filter, 10, 0).await.unwrap();
        assert_eq!(filtered.len(), 1, "{status}");
        assert_eq!(filtered[0].id, expected.id, "{status}");
        assert_eq!(
            repo.count_dags_by_display_status(&filter).await.unwrap(),
            1,
            "{status}"
        );
        assert_eq!(
            repo.get_dag_display_status(&expected.id).await.unwrap(),
            status
        );
        assert_eq!(
            repo.get_dag_with_display_status(&expected.id)
                .await
                .unwrap()
                .1,
            status
        );
    }

    assert_eq!(
        repo.get_dag_display_counts().await.unwrap(),
        DagDisplayCounts {
            pending: 1,
            processing: 1,
            completed: 1,
            failed: 0,
            cancelled: 0,
            avg_duration_secs: Some(4.0),
        }
    );

    // Terminal history is counted and averaged from the status index alone.
    for query in [DAG_DISPLAY_COUNTS_SQL, DAG_AVG_DURATION_SQL] {
        let plan: Vec<(i64, i64, i64, String)> =
            sqlx::query_as(sqlx::AssertSqlSafe(format!("EXPLAIN QUERY PLAN {query}")))
                .fetch_all(&pool)
                .await
                .unwrap();
        let plan: Vec<String> = plan.into_iter().map(|(.., detail)| detail).collect();
        assert!(
            plan.iter().any(|detail| detail
                .contains("USING COVERING INDEX idx_dag_execution_status_created_completed")),
            "{query}: {plan:?}"
        );
    }
}

#[tokio::test]
async fn dag_summaries_parse_names_and_keep_list_order_and_filters() {
    let pool = setup_test_pool().await;
    let repo = SqlxDagRepository::new(pool.clone(), pool.clone());
    let jobs = SqlxJobRepository::new(pool.clone(), pool.clone());
    let definitions = [
        (
            r#"{"name":"Archive_Backup%\\","steps":[]}"#,
            Some("Archive_Backup%\\"),
        ),
        (r#"{"steps":[]}"#, None),
        (r#"{"name":42}"#, None),
        ("{invalid", None),
        (r#"{"name":"first","name":"second"}"#, None),
        (r#"{"name":"Finished"}"#, Some("Finished")),
    ];
    let mut expected = Vec::new();
    for (index, (definition, name)) in definitions.iter().enumerate() {
        let mut dag = DagExecutionDbModel::new(
            &DagPipelineDefinition::new("placeholder", vec![]),
            Some(format!("streamer-{index}")),
            Some(format!("session-{}", index % 2)),
        );
        dag.id = format!("summary-{index}");
        dag.dag_definition = (*definition).to_owned();
        dag.created_at = index as i64;
        dag.updated_at = index as i64 + 10;
        dag.status = [
            "PENDING",
            "PROCESSING",
            "COMPLETED",
            "FAILED",
            "CANCELLED",
            "COMPLETED",
        ][index]
            .to_owned();
        dag.total_steps = index as i32;
        dag.completed_steps = index as i32 / 2;
        dag.failed_steps = index as i32 % 2;
        dag.input_manifest = Some("unused manifest payload".to_owned());
        dag.error = Some("unused error payload".to_owned());
        repo.create_dag(&dag).await.unwrap();
        if index == 1 {
            let mut job = JobDbModel::new("remux", "{}");
            job.status = "PROCESSING".to_owned();
            job.pipeline_id = Some(dag.id.clone());
            jobs.create_job(&job).await.unwrap();
        }
        expected.push(DagExecutionListSummary {
            display_status: repo.get_dag_display_status(&dag.id).await.unwrap(),
            id: dag.id,
            name: name.map(str::to_owned),
            streamer_id: dag.streamer_id,
            session_id: dag.session_id,
            total_steps: dag.total_steps,
            completed_steps: dag.completed_steps,
            failed_steps: dag.failed_steps,
            created_at: dag.created_at,
            updated_at: dag.updated_at,
        });
    }
    expected.reverse();

    let all = DagListFilter::default();
    assert_eq!(
        repo.list_dag_summaries(&all, 10, 0).await.unwrap(),
        expected
    );
    assert_eq!(
        repo.list_dag_summaries(&all, 2, 1).await.unwrap(),
        expected[1..3]
    );
    let session = DagListFilter {
        session_id: Some("session-1"),
        ..Default::default()
    };
    let in_session: Vec<_> = expected
        .iter()
        .filter(|dag| dag.session_id.as_deref() == Some("session-1"))
        .cloned()
        .collect();
    assert_eq!(
        repo.list_dag_summaries(&session, 10, 0).await.unwrap(),
        in_session
    );
    // Search uses SQLite JSON extraction; malformed definitions keep its
    // query error rather than being silently repaired.
    let search = DagListFilter {
        search: Some("missing"),
        ..Default::default()
    };
    assert!(repo.list_dag_summaries(&search, 10, 0).await.is_err());
}

async fn search_dag_ids(
    repo: &SqlxDagRepository,
    search: &str,
    status: Option<DagExecutionStatus>,
) -> Vec<String> {
    let filter = DagListFilter {
        status,
        search: Some(search),
        ..Default::default()
    };
    let mut ids: Vec<String> = repo
        .list_dag_summaries(&filter, 10, 0)
        .await
        .unwrap()
        .into_iter()
        .map(|dag| dag.id)
        .collect();
    ids.sort();
    assert_eq!(
        repo.count_dags_by_display_status(&filter).await.unwrap(),
        ids.len() as u64
    );
    ids
}

/// Search matches a case-insensitive substring of the DAG name or ID, treats
/// LIKE wildcards literally, and combines with the status filter.
#[tokio::test]
async fn dag_list_search_matches_name_and_id_literally() {
    let pool = setup_test_pool().await;
    let repo = SqlxDagRepository::new(pool.clone(), pool.clone());
    let upload = DagExecutionDbModel::new(
        &DagPipelineDefinition::new(
            "Upload_Backup",
            vec![DagStep::new("A", PipelineStep::preset("remux"))],
        ),
        None,
        Some("search-session".to_owned()),
    );
    let mut remux = DagExecutionDbModel::new(
        &DagPipelineDefinition::new(
            "UploadXBackup",
            vec![DagStep::new("A", PipelineStep::preset("remux"))],
        ),
        None,
        None,
    );
    remux.status = DagExecutionStatus::Completed.as_str().to_string();
    repo.create_dag(&upload).await.unwrap();
    repo.create_dag(&remux).await.unwrap();

    let mut both = vec![upload.id.clone(), remux.id.clone()];
    both.sort();
    assert_eq!(search_dag_ids(&repo, "upload", None).await, both);
    assert_eq!(search_dag_ids(&repo, "  ", None).await, both);
    assert_eq!(
        search_dag_ids(&repo, "d_b", None).await,
        vec![upload.id.clone()]
    );
    assert_eq!(
        search_dag_ids(&repo, &upload.id[..8], None).await,
        vec![upload.id.clone()]
    );
    assert_eq!(
        search_dag_ids(&repo, "upload", Some(DagExecutionStatus::Completed)).await,
        vec![remux.id.clone()]
    );
    assert!(search_dag_ids(&repo, "missing", None).await.is_empty());
    let filter = DagListFilter {
        status: Some(DagExecutionStatus::Pending),
        session_id: Some("search-session"),
        search: Some("  d_b  "),
    };
    let rows = repo.list_dag_summaries(&filter, 10, 0).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, upload.id);
    assert_eq!(repo.count_dags_by_display_status(&filter).await.unwrap(), 1);
}

/// The no-job completion transitions only a ready step that has no job in a
/// non-terminal DAG; anything else is reported like a duplicate completion.
#[tokio::test]
async fn complete_ready_step_without_job_guards_its_transition() {
    let pool = setup_test_pool().await;
    let repo = SqlxDagRepository::new(pool.clone(), pool.clone());
    let dag_def = DagPipelineDefinition::new(
        "no-op",
        vec![
            DagStep::new("A", PipelineStep::preset("remux")),
            DagStep::with_dependencies("B", PipelineStep::preset("delete"), vec!["A".to_string()]),
        ],
    );
    let mut dag = DagExecutionDbModel::new(&dag_def, None, None);
    dag.status = DagExecutionStatus::Processing.as_str().to_string();
    repo.create_dag(&dag).await.unwrap();
    let step_a = DagStepExecutionDbModel::new(&dag.id, "A", &[]);
    let step_b = DagStepExecutionDbModel::new(&dag.id, "B", &["A".to_string()]);
    repo.create_steps(&[step_a.clone(), step_b.clone()])
        .await
        .unwrap();
    let mut job = JobDbModel::new_pipeline_step("remux", "[]", "[]", 0, None, None);
    job.dag_step_execution_id = Some(step_a.id.clone());
    repo.create_job_for_step(&step_a.id, &job).await.unwrap();

    // A step with a job keeps its status.
    let untouched = repo
        .complete_ready_step_without_job(&step_a.id, &[])
        .await
        .unwrap();
    assert_eq!(untouched.step.status, "PROCESSING");
    assert!(untouched.ready_steps.is_empty());
    assert_eq!(untouched.dag.completed_steps, 0);

    repo.complete_step_and_check_dependents(&step_a.id, &[])
        .await
        .unwrap();
    let completed = repo
        .complete_ready_step_without_job(&step_b.id, &[])
        .await
        .unwrap();
    assert_eq!(completed.step.status, "COMPLETED");
    assert_eq!(completed.step.get_outputs(), Vec::<String>::new());
    assert_eq!(completed.dag.completed_steps, 2);
    assert_eq!(completed.dag.status, "COMPLETED");

    let replay = repo
        .complete_ready_step_without_job(&step_b.id, &["late.mp4".to_string()])
        .await
        .unwrap();
    assert_eq!(replay.step.get_outputs(), Vec::<String>::new());
    assert_eq!(replay.dag.completed_steps, 2);
}

#[tokio::test]
async fn test_fan_in_complete() {
    let pool = setup_test_pool().await;
    let repo = SqlxDagRepository::new(pool.clone(), pool.clone());

    // Create DAG: [A, B] -> C (fan-in)
    let dag_def = DagPipelineDefinition::new(
        "fan-in-dag",
        vec![
            DagStep::new("A", PipelineStep::preset("remux")),
            DagStep::new("B", PipelineStep::preset("thumbnail")),
            DagStep::with_dependencies(
                "C",
                PipelineStep::preset("upload"),
                vec!["A".to_string(), "B".to_string()],
            ),
        ],
    );

    let dag = DagExecutionDbModel::new(&dag_def, None, None);
    let dag_id = dag.id.clone();
    repo.create_dag(&dag).await.unwrap();

    let step_a = DagStepExecutionDbModel::new(&dag_id, "A", &[]);
    let step_a_id = step_a.id.clone();
    let step_b = DagStepExecutionDbModel::new(&dag_id, "B", &[]);
    let step_b_id = step_b.id.clone();
    let step_c = DagStepExecutionDbModel::new(&dag_id, "C", &["A".to_string(), "B".to_string()]);

    repo.create_steps(&[step_a, step_b, step_c]).await.unwrap();

    // Complete step A - C should NOT be ready yet
    let ready = repo
        .complete_step_and_check_dependents(&step_a_id, &["/output/a.mp4".to_string()])
        .await
        .unwrap()
        .ready_steps;
    assert!(ready.is_empty());

    // Complete step B - NOW C should be ready with merged inputs
    let ready = repo
        .complete_step_and_check_dependents(&step_b_id, &["/output/b.jpg".to_string()])
        .await
        .unwrap()
        .ready_steps;
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].step.step_id, "C");
    assert_eq!(
        ready[0].merged_inputs,
        vec!["/output/a.mp4".to_string(), "/output/b.jpg".to_string()]
    );
}

#[tokio::test]
async fn test_complete_step_is_idempotent() {
    let pool = setup_test_pool().await;
    let repo = SqlxDagRepository::new(pool.clone(), pool.clone());

    // Create DAG: A -> B
    let dag_def = DagPipelineDefinition::new(
        "test-dag",
        vec![
            DagStep::new("A", PipelineStep::preset("remux")),
            DagStep::with_dependencies("B", PipelineStep::preset("upload"), vec!["A".to_string()]),
        ],
    );

    let dag = DagExecutionDbModel::new(&dag_def, None, None);
    let dag_id = dag.id.clone();
    repo.create_dag(&dag).await.unwrap();

    let step_a = DagStepExecutionDbModel::new(&dag_id, "A", &[]);
    let step_a_id = step_a.id.clone();
    let step_b = DagStepExecutionDbModel::new(&dag_id, "B", &["A".to_string()]);

    repo.create_steps(&[step_a, step_b]).await.unwrap();

    let ready = repo
        .complete_step_and_check_dependents(&step_a_id, &["/output/a.mp4".to_string()])
        .await
        .unwrap()
        .ready_steps;
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].step.step_id, "B");

    // Duplicate completion should be a no-op (no double increment, no re-ready).
    let ready2 = repo
        .complete_step_and_check_dependents(&step_a_id, &["/output/a.mp4".to_string()])
        .await
        .unwrap()
        .ready_steps;
    assert!(ready2.is_empty());

    let dag = repo.get_dag(&dag_id).await.unwrap();
    assert_eq!(dag.completed_steps, 1);

    let step_b = repo
        .get_step_by_dag_and_step_id(&dag_id, "B")
        .await
        .unwrap();
    assert_eq!(step_b.status, "PENDING");
}

#[tokio::test]
async fn test_fail_dag_cancels_processing_steps() {
    let pool = setup_test_pool().await;
    let repo = SqlxDagRepository::new(pool.clone(), pool.clone());

    // Create DAG: A -> B
    let dag_def = DagPipelineDefinition::new(
        "test-dag",
        vec![
            DagStep::new("A", PipelineStep::preset("remux")),
            DagStep::with_dependencies("B", PipelineStep::preset("upload"), vec!["A".to_string()]),
        ],
    );

    let dag = DagExecutionDbModel::new(&dag_def, None, None);
    let dag_id = dag.id.clone();
    repo.create_dag(&dag).await.unwrap();

    let step_a = DagStepExecutionDbModel::new(&dag_id, "A", &[]);
    let mut step_b = DagStepExecutionDbModel::new(&dag_id, "B", &["A".to_string()]);

    // Simulate an in-flight job for step B.
    create_job(&pool, "job-b").await;
    step_b.status = "PROCESSING".to_string();
    step_b.job_id = Some("job-b".to_string());
    repo.create_steps(&[step_a, step_b]).await.unwrap();

    let cancelled = repo
        .fail_dag_and_cancel_steps(&dag_id, "fail-fast")
        .await
        .unwrap();
    assert_eq!(cancelled, vec!["job-b".to_string()]);

    let step_b = repo
        .get_step_by_dag_and_step_id(&dag_id, "B")
        .await
        .unwrap();
    assert_eq!(step_b.status, "CANCELLED");

    let dag = repo.get_dag(&dag_id).await.unwrap();
    assert_eq!(dag.status, "FAILED");
}

/// Rows for a DAG whose steps are in the given states, with a job row
/// attached where a job id is given. Returns the step execution ids.
async fn seed_dag_rows(
    pool: &SqlitePool,
    repo: &SqlxDagRepository,
    definition: &DagPipelineDefinition,
    steps: &[(&str, &str, Option<&str>)],
) -> (DagExecutionDbModel, HashMap<String, String>) {
    let mut dag = DagExecutionDbModel::new(definition, None, None);
    dag.status = "PROCESSING".to_string();
    repo.create_dag(&dag).await.unwrap();
    let mut ids = HashMap::new();
    for (step_id, status, job_id) in steps {
        let depends_on = definition.get_step(step_id).unwrap().depends_on.clone();
        let mut step = DagStepExecutionDbModel::new(&dag.id, step_id, &depends_on);
        step.status = status.to_string();
        if let Some(job_id) = job_id {
            create_job(pool, job_id).await;
            step.job_id = Some(job_id.to_string());
        }
        repo.create_step(&step).await.unwrap();
        ids.insert(step_id.to_string(), step.id);
    }
    (dag, ids)
}

fn noop_definition(name: &str, steps: &[(&str, &[&str])]) -> DagPipelineDefinition {
    DagPipelineDefinition::new(
        name,
        steps
            .iter()
            .map(|(id, depends_on)| {
                DagStep::with_dependencies(
                    *id,
                    PipelineStep::preset("noop"),
                    depends_on.iter().map(|dep| dep.to_string()).collect(),
                )
            })
            .collect(),
    )
}

async fn step_statuses(repo: &SqlxDagRepository, dag_id: &str) -> Vec<String> {
    let mut statuses: Vec<String> = repo
        .get_steps_by_dag(dag_id)
        .await
        .unwrap()
        .into_iter()
        .map(|step| format!("{}={}", step.step_id, step.status))
        .collect();
    statuses.sort();
    statuses
}

/// Failing a step cancels the steps that depend on it, directly or through
/// others, and returns the jobs of those already running. Independent
/// branches stay active, the first error is kept, and the DAG is finalized
/// only by the last branch to settle.
#[tokio::test]
async fn fail_step_cancels_transitive_dependents_and_keeps_independent_branches() {
    let pool = setup_test_pool().await;
    let repo = SqlxDagRepository::new(pool.clone(), pool.clone());
    let definition = noop_definition(
        "scoped fail-fast",
        &[
            ("A", &[]),
            ("B", &["A"]),
            ("C", &["A"]),
            ("D", &["B", "C"]),
            ("E", &["D"]),
            ("F", &[]),
        ],
    );
    // D running while its dependency B is still active only arises from a
    // race, but its job must then be cancelled like any dependent.
    let (dag, ids) = seed_dag_rows(
        &pool,
        &repo,
        &definition,
        &[
            ("A", "COMPLETED", None),
            ("B", "PROCESSING", Some("job-b")),
            ("C", "PROCESSING", Some("job-c")),
            ("D", "PROCESSING", Some("job-d")),
            ("E", "BLOCKED", None),
            ("F", "PROCESSING", Some("job-f")),
        ],
    )
    .await;

    let cancelled = repo
        .fail_step_and_cancel_dag(&ids["B"], "B broke")
        .await
        .unwrap()
        .expect("B was active");
    assert_eq!(cancelled, vec!["job-d".to_string()]);
    assert_eq!(
        step_statuses(&repo, &dag.id).await,
        [
            "A=COMPLETED",
            "B=FAILED",
            "C=PROCESSING",
            "D=CANCELLED",
            "E=CANCELLED",
            "F=PROCESSING"
        ]
    );
    let draining = repo.get_dag(&dag.id).await.unwrap();
    assert_eq!(draining.status, "PROCESSING");
    assert_eq!(draining.failed_steps, 1);
    assert_eq!(draining.error.as_deref(), Some("B broke"));
    assert!(draining.completed_at.is_none());

    // A second failure on the draining DAG keeps the first error and has
    // nothing left to cancel.
    let cancelled = repo
        .fail_step_and_cancel_dag(&ids["C"], "C broke")
        .await
        .unwrap()
        .expect("C was active");
    assert!(cancelled.is_empty());
    let draining = repo.get_dag(&dag.id).await.unwrap();
    assert_eq!(draining.status, "PROCESSING");
    assert_eq!(draining.failed_steps, 2);
    assert_eq!(draining.error.as_deref(), Some("B broke"));

    // A step that already settled is not failed again.
    assert!(
        repo.fail_step_and_cancel_dag(&ids["D"], "late")
            .await
            .unwrap()
            .is_none()
    );

    // The independent branch settles the DAG.
    let completion = repo
        .complete_step_and_check_dependents(&ids["F"], &[])
        .await
        .unwrap();
    assert!(completion.ready_steps.is_empty());
    assert_eq!(completion.dag.status, "FAILED");
    assert!(completion.dag.completed_at.is_some());
    assert_eq!(completion.dag.completed_steps, 1);
    assert_eq!(completion.dag.failed_steps, 2);
    assert_eq!(completion.dag.error.as_deref(), Some("B broke"));
}

/// Startup recovery materializes only a jobless step whose dependencies
/// have all completed. One still waiting on a running dependency is not a
/// candidate, so a DAG in normal flight is not re-read at every start.
/// Readiness for retry fan-out and recovery is the completion
/// transaction's readiness asked for the whole DAG: a jobless BLOCKED or
/// PENDING step whose dependencies have all COMPLETED, with its inputs
/// merged in dependency order and each path once. Steps with a job, with
/// a dependency that has not completed, and roots are not listed.
#[tokio::test]
async fn list_ready_steps_matches_the_completion_transactions_readiness() {
    let pool = setup_test_pool().await;
    let repo = SqlxDagRepository::new(pool.clone(), pool.clone());
    let definition = noop_definition(
        "readiness",
        &[
            ("A", &[]),
            ("B", &[]),
            ("C", &["A", "B"]),
            ("D", &["A"]),
            ("E", &["C"]),
            ("F", &["B"]),
            ("G", &[]),
        ],
    );
    let (dag, ids) = seed_dag_rows(
        &pool,
        &repo,
        &definition,
        &[
            ("A", "COMPLETED", Some("job-a")),
            ("B", "COMPLETED", Some("job-b")),
            ("C", "BLOCKED", None),
            ("D", "PENDING", None),
            ("E", "BLOCKED", None),
            ("F", "PROCESSING", Some("job-f")),
            ("G", "PENDING", None),
        ],
    )
    .await;
    for (step, outputs) in [
        ("A", r#"["/out/a.mp4","/out/shared.xml"]"#),
        ("B", r#"["/out/shared.xml","/out/b.mp4"]"#),
    ] {
        sqlx::query("UPDATE dag_step_execution SET outputs = ? WHERE id = ?")
            .bind(outputs)
            .bind(&ids[step])
            .execute(&pool)
            .await
            .unwrap();
    }

    let ready = repo.list_ready_steps(&dag.id).await.unwrap();
    let listed: Vec<(&str, &[String])> = ready
        .iter()
        .map(|ready| (ready.step.step_id.as_str(), ready.merged_inputs.as_slice()))
        .collect();
    assert_eq!(
        listed,
        [
            (
                "C",
                &[
                    "/out/a.mp4".to_string(),
                    "/out/shared.xml".to_string(),
                    "/out/b.mp4".to_string()
                ][..]
            ),
            (
                "D",
                &["/out/a.mp4".to_string(), "/out/shared.xml".to_string()][..]
            ),
        ]
    );

    // Completing B through the transaction offers F's readiness the same
    // way, for the blocked dependents of B only.
    sqlx::query(
        "UPDATE dag_step_execution SET status = 'PROCESSING', job_id = 'job-b' WHERE id = ?",
    )
    .bind(&ids["B"])
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE dag_step_execution SET status = 'BLOCKED', job_id = NULL WHERE id = ?")
        .bind(&ids["F"])
        .execute(&pool)
        .await
        .unwrap();
    let completion = repo
        .complete_step_and_check_dependents(
            &ids["B"],
            &["/out/shared.xml".to_string(), "/out/b.mp4".to_string()],
        )
        .await
        .unwrap();
    let mut offered: Vec<(&str, &[String])> = completion
        .ready_steps
        .iter()
        .map(|ready| (ready.step.step_id.as_str(), ready.merged_inputs.as_slice()))
        .collect();
    offered.sort();
    assert_eq!(
        offered,
        [
            (
                "C",
                &[
                    "/out/a.mp4".to_string(),
                    "/out/shared.xml".to_string(),
                    "/out/b.mp4".to_string()
                ][..]
            ),
            (
                "F",
                &["/out/shared.xml".to_string(), "/out/b.mp4".to_string()][..]
            ),
        ],
        "D was already PENDING and is not B's dependent"
    );
    assert!(
        completion
            .ready_steps
            .iter()
            .all(|ready| ready.step.status == "PENDING")
    );
}

#[tokio::test]
async fn unmaterialized_step_query_skips_steps_with_a_running_dependency() {
    let pool = setup_test_pool().await;
    let repo = SqlxDagRepository::new(pool.clone(), pool.clone());
    let definition = noop_definition(
        "waiting fan-in",
        &[("A", &[]), ("B", &["A"]), ("C", &["A"]), ("D", &["B", "C"])],
    );
    let (dag, ids) = seed_dag_rows(
        &pool,
        &repo,
        &definition,
        &[
            ("A", "COMPLETED", None),
            ("B", "COMPLETED", None),
            ("C", "PROCESSING", Some("job-c")),
            ("D", "BLOCKED", None),
        ],
    )
    .await;

    assert!(
        repo.list_dag_ids_with_unmaterialized_steps()
            .await
            .unwrap()
            .is_empty(),
        "D still waits on C"
    );

    sqlx::query("UPDATE dag_step_execution SET status = 'COMPLETED' WHERE id = ?")
        .bind(&ids["C"])
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.list_dag_ids_with_unmaterialized_steps().await.unwrap(),
        vec![dag.id.clone()],
        "D is ready once every dependency completed"
    );
}

/// A step whose job failed but waits for a scheduled automatic retry is
/// the retry sweeper's to advance, not startup reconciliation's.
#[tokio::test]
async fn stranded_step_query_skips_jobs_with_a_scheduled_retry() {
    let pool = setup_test_pool().await;
    let repo = SqlxDagRepository::new(pool.clone(), pool.clone());
    let jobs = SqlxJobRepository::new(pool.clone(), pool.clone());
    let definition = noop_definition("retrying", &[("A", &[]), ("B", &[])]);
    let (_dag, ids) = seed_dag_rows(
        &pool,
        &repo,
        &definition,
        &[
            ("A", "PROCESSING", Some("job-a")),
            ("B", "PROCESSING", Some("job-b")),
        ],
    )
    .await;
    assert_eq!(
        jobs.mark_job_failed("job-a", "boom", None).await.unwrap(),
        1
    );
    assert_eq!(
        jobs.mark_job_failed("job-b", "boom", Some(i64::MAX))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        repo.list_processing_steps_with_failed_jobs()
            .await
            .unwrap()
            .iter()
            .map(|step| step.id.as_str())
            .collect::<Vec<_>>(),
        vec![ids["A"].as_str()],
        "a failed job without a retry is stranded; one with a scheduled retry is not"
    );
}
