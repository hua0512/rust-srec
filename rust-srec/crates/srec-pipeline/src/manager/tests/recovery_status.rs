use super::*;
use crate::repositories::{SqlxDagRepository, SqlxJobRepository};

async fn pool() -> sqlx::SqlitePool {
    let pool = srec_sqlite::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    srec_sqlite::run_migrations(&pool).await.unwrap();
    pool
}

#[tokio::test]
async fn recovery_report_confirms_success_but_retains_partial_dag_recovery_failure() {
    for fail_dag in [false, true] {
        let pool = pool().await;
        let jobs = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
        let mut job = JobDbModel::new_with_input("remux", "/input.flv", 0, None, None, "{}");
        job.status = JobStatus::Processing.as_str().to_owned();
        jobs.create_job(&job).await.unwrap();
        let dag_pool = if fail_dag {
            let closed = self::pool().await;
            closed.close().await;
            closed
        } else {
            pool.clone()
        };
        let manager: PipelineManager =
            PipelineManager::with_repository(PipelineManagerConfig::default(), jobs.clone())
                .with_dag_repository(Arc::new(SqlxDagRepository::new(dag_pool.clone(), dag_pool)));
        let report = manager.recover_jobs_with_status().await.unwrap();
        assert_eq!(report.recovered_jobs, 1);
        assert_eq!(report.complete, !fail_dag);
        assert_eq!(
            jobs.get_job(&job.id).await.unwrap().status,
            JobStatus::Pending.as_str(),
            "partial reconciliation must not prevent interrupted-job reset"
        );
        pool.close().await;
    }
}

#[tokio::test]
async fn recovery_report_does_not_hide_per_job_persistence_failure() {
    let pool = pool().await;
    let jobs = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
    // One job more than a sweep page, none of which can be failed: the sweep has to move
    // past the jobs it leaves pending rather than list the same page again.
    let unfailable = u64::from(recovery::COORDINATOR_RECOVERY_PAGE_LIMIT) + 1;
    sqlx::query(
        "WITH RECURSIVE backlog(n) AS (
             VALUES (1) UNION ALL SELECT n + 1 FROM backlog WHERE n < ?
         )
         INSERT INTO job (id, job_type, status, config, state, created_at, updated_at)
         SELECT 'unfailable-' || n, 'missing-processor', 'PENDING', '{}', '{}', n, n FROM backlog",
    )
    .bind(unfailable.cast_signed())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER reject_recovery_failure BEFORE UPDATE OF status ON job WHEN NEW.status = 'FAILED' BEGIN SELECT RAISE(ABORT, 'injected failure'); END")
        .execute(&pool).await.unwrap();
    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), jobs.clone());
    let report = tokio::time::timeout(Duration::from_secs(60), manager.recover_jobs_with_status())
        .await
        .expect("the sweep must end when jobs cannot be failed")
        .unwrap();
    assert!(!report.complete);
    assert_eq!(jobs.count_pending_jobs(None).await.unwrap(), unfailable);
    pool.close().await;
}

#[tokio::test]
async fn recovery_fails_unsupported_job_behind_large_pending_backlog() {
    let pool = pool().await;
    let jobs = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
    sqlx::query(
        "WITH RECURSIVE backlog(n) AS (
             VALUES (1) UNION ALL SELECT n + 1 FROM backlog WHERE n < 10000
         )
         INSERT INTO job (id, job_type, status, config, state, created_at, updated_at)
         SELECT 'supported-' || n, 'remux', 'PENDING', '{}', '{}', n, n FROM backlog",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut unsupported = JobDbModel::new("missing-processor", "{}");
    unsupported.created_at = 0;
    jobs.create_job(&unsupported).await.unwrap();
    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), jobs.clone());

    let report = manager.recover_jobs_with_status().await.unwrap();
    assert!(report.complete);
    assert_eq!(
        jobs.get_job(&unsupported.id).await.unwrap().status,
        JobStatus::Failed.as_str()
    );
    assert_eq!(jobs.count_pending_jobs(None).await.unwrap(), 10_000);
}

#[tokio::test]
async fn recovery_command_report_tracks_real_publication_outcomes() {
    for fail in [false, true] {
        let mut repository = TestDagRepository::new();
        repository.before_publish = Some(Box::new(move |_| {
            if fail {
                Err(DbError::database("injected publication failure"))
            } else {
                Ok(())
            }
        }));
        let repository = Arc::new(repository);
        let manager: PipelineManager = PipelineManager::with_repository(
            PipelineManagerConfig::default(),
            Arc::new(TestJobRepository::new()),
        )
        .with_dag_repository(repository.clone());
        let succeeded = manager
            .execute_pipeline_commands_with_status(vec![PipelineCommand::CreateSegmentDag {
                session_id: "session".to_owned(),
                streamer_id: "streamer".to_owned(),
                segment_index: 0,
                source: SourceType::Video,
                input_path: PathBuf::from("input.flv"),
                pipeline: DagPipelineDefinition::new(
                    "recovery",
                    vec![DagStep::new(
                        "root",
                        PipelineStep::inline("remux", serde_json::json!({})),
                    )],
                ),
            }])
            .await;
        assert_eq!(succeeded, !fail);
        assert_eq!(repository.create_calls(), usize::from(!fail));
    }
}

#[tokio::test]
async fn recovery_confirmation_includes_video_segments_beyond_the_old_cap() {
    const SEGMENTS: u32 = 10_001;
    let pool = pool().await;
    let jobs = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
    let sessions = Arc::new(TestSessionRepository::new(None));
    sessions.insert_session(test_session("long-session", "streamer", None));
    for index in 0..SEGMENTS {
        sessions.insert_segment(test_segment(
            "long-session",
            index,
            &format!("clip-{index}.flv"),
        ));
    }
    let manager = PipelineManager::with_repository(PipelineManagerConfig::default(), jobs)
        .with_session_port(session_port(sessions));
    let report = manager.recover_jobs_with_status().await.unwrap();
    assert!(report.complete);
    manager
        .pipeline_coordinator
        .apply_event(PipelineCoordinationEvent::ConfigureSession {
            session_id: "long-session".to_owned(),
            streamer_id: "streamer".to_owned(),
            danmu_enabled: false,
            segment_pipeline: None,
            paired_segment_pipeline: None,
            session_complete_pipeline: Some(DagPipelineDefinition::new(
                "complete",
                vec![DagStep::new("step", PipelineStep::preset("remux"))],
            )),
        });
    manager
        .pipeline_coordinator
        .apply_event(PipelineCoordinationEvent::SessionEnded {
            session_id: "long-session".to_owned(),
            streamer_id: "streamer".to_owned(),
            should_run_session_complete: true,
        });
    let commands =
        manager
            .pipeline_coordinator
            .apply_event(PipelineCoordinationEvent::SessionEndPersisted {
                session_id: "long-session".to_owned(),
            });
    let [PipelineCommand::CreateSessionCompleteDag { outputs, .. }] = commands.as_slice() else {
        panic!("expected one session completion command");
    };
    assert_eq!(outputs.video_outputs.len(), SEGMENTS as usize);
    assert!(
        outputs
            .video_outputs
            .iter()
            .any(|output| output.segment_index == 10_000)
    );
    pool.close().await;
}
