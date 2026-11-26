use std::collections::HashMap;

use sqlx::SqlitePool;

use super::*;
use crate::repositories::{JobRepository as _, SqlxJobRepository};
use srec_sqlite::models::{
    DagDisplayCounts, DagExecutionDbModel, DagExecutionStatus, DagPipelineDefinition, DagStep,
    DagStepExecutionDbModel, JobDbModel, PipelineStep,
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
    let mut running = DagExecutionDbModel::new(&dag_def, None, Some("session".to_string()));
    running.status = DagExecutionStatus::Processing.as_str().to_string();
    running.created_at = 2_000;
    running.input_manifest = Some(r#"{"version":1}"#.to_string());
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

    // Drain and coordinator recovery filter the stored rows by status and session.
    let listed = repo.list_dags(None, Some("session"), 10, 0).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, running.id);
    assert_eq!(listed[0].input_manifest, running.input_manifest);
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

    let first = repo
        .complete_step_and_check_dependents(&step_a.id, &[])
        .await
        .unwrap();
    assert_eq!(first.ready_steps.len(), 1);
    assert_eq!(first.ready_steps[0].step.step_id, "B");
    // A repeated completion of a step with a job neither counts again nor
    // offers its dependents again.
    let duplicate = repo
        .complete_step_and_check_dependents(&step_a.id, &["/output/a.mp4".to_string()])
        .await
        .unwrap();
    assert!(duplicate.ready_steps.is_empty());
    assert_eq!(repo.get_dag(&dag.id).await.unwrap().completed_steps, 1);
    assert_eq!(
        repo.get_step_by_dag_and_step_id(&dag.id, "B")
            .await
            .unwrap()
            .status,
        "PENDING"
    );

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

    // A DAG whose only jobless step still waits on a running dependency.
    let waiting_definition = noop_definition(
        "waiting fan-in",
        &[("A", &[]), ("B", &["A"]), ("C", &["A"]), ("D", &["B", "C"])],
    );
    let (waiting, waiting_ids) = seed_dag_rows(
        &pool,
        &repo,
        &waiting_definition,
        &[
            ("A", "COMPLETED", None),
            ("B", "COMPLETED", None),
            ("C", "PROCESSING", Some("job-waiting-c")),
            ("D", "BLOCKED", None),
        ],
    )
    .await;
    assert_eq!(
        repo.list_dag_ids_with_unmaterialized_steps().await.unwrap(),
        vec![dag.id.clone()],
        "the waiting DAG's D still waits on C"
    );
    sqlx::query("UPDATE dag_step_execution SET status = 'COMPLETED' WHERE id = ?")
        .bind(&waiting_ids["C"])
        .execute(&pool)
        .await
        .unwrap();
    let mut unmaterialized = repo.list_dag_ids_with_unmaterialized_steps().await.unwrap();
    unmaterialized.sort();
    let mut both = vec![dag.id.clone(), waiting.id.clone()];
    both.sort();
    assert_eq!(
        unmaterialized, both,
        "D is ready once every dependency completed"
    );

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
