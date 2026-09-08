use super::*;

use crate::database::models::StreamerDbModel;
use crate::database::repositories::{SqlxDagRepository, SqlxJobRepository, SqlxSessionRepository};
use crate::database::test_support::SqlTrace;

async fn database() -> sqlx::SqlitePool {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    pool
}

#[tokio::test]
async fn constructors_share_processor_and_throttle_contracts_without_losing_persistence() {
    let pool = database().await;
    for enabled in [false, true] {
        let mut config = PipelineManagerConfig::default();
        config.throttle.enabled = enabled;
        config.throttle.critical_threshold = 321;
        let memory: PipelineManager = PipelineManager::with_config(config.clone());
        let persistent: PipelineManager = PipelineManager::with_repository(
            config,
            Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone())),
        );
        assert_eq!(
            memory.supported_job_types(),
            persistent.supported_job_types()
        );
        assert!(persistent.supported_job_types().contains("execute"));
        for manager in [&memory, &persistent] {
            assert_eq!(manager.throttle_controller().is_some(), enabled);
            if let Some(throttle) = manager.throttle_controller() {
                assert_eq!(throttle.config().critical_threshold, 321);
            }
            assert!(!manager.is_throttled());
            manager
                .job_queue
                .enqueue(Job::new("execute", vec![], vec![], "streamer", "session"))
                .await
                .unwrap();
            assert_eq!(manager.queue_depth(), 1);
        }
    }
    let persisted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM job")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(persisted, 2);
    pool.close().await;
}

#[tokio::test]
async fn dag_creation_reads_streamer_once_and_persists_all_placeholder_metadata() {
    let pool = database().await;
    let streamers = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
    let streamer = StreamerDbModel::new(
        "Recorded streamer",
        "https://example.com/live",
        "platform-twitch",
    );
    streamers.create_streamer(&streamer).await.unwrap();
    let sessions = Arc::new(SqlxSessionRepository::new(pool.clone(), pool.clone()));
    let mut session = LiveSessionDbModel::new(&streamer.id);
    session.start_time = 1_700_000_000_000;
    session.titles =
        Some(serde_json::json!([{"ts": session.start_time, "title": "Current title"}]).to_string());
    sessions.create_session(&session).await.unwrap();
    let configs = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
    let expected_platform = configs
        .get_platform_config("platform-twitch")
        .await
        .unwrap()
        .platform_name;
    let service = Arc::new(ConfigService::new(configs, streamers.clone()));
    let jobs = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
    let manager: PipelineManager =
        PipelineManager::with_repository(PipelineManagerConfig::default(), jobs.clone())
            .with_session_repository(sessions)
            .with_streamer_repository(streamers)
            .with_config_service(service)
            .with_dag_repository(Arc::new(SqlxDagRepository::new(pool.clone(), pool.clone())));
    let trace = SqlTrace::install(&pool).await;
    let created = manager
        .create_dag_pipeline(
            &session.id,
            &streamer.id,
            vec!["input.flv".to_owned()],
            DagPipelineDefinition::new(
                "metadata",
                vec![DagStep::new(
                    "root",
                    PipelineStep::inline("remux", serde_json::json!({})),
                )],
            ),
        )
        .await
        .unwrap();
    let statements = trace.statements();
    assert_eq!(
        statements
            .iter()
            .filter(|sql| sql.starts_with("SELECT * FROM streamers WHERE id ="))
            .count(),
        1,
        "{statements:?}"
    );
    let job = jobs.get_job(&created.root_job_ids[0]).await.unwrap();
    let state: serde_json::Value = serde_json::from_str(&job.state).unwrap();
    assert_eq!(state["streamer_name"], "Recorded streamer");
    assert_eq!(state["session_title"], "Current title");
    assert_eq!(state["platform"], expected_platform);
    assert_eq!(state["session_start_ms"], session.start_time);
    pool.close().await;
}

#[tokio::test]
async fn coordinator_recovery_pages_all_session_statuses_in_one_scan() {
    let pool = database().await;
    let streamers = SqlxStreamerRepository::new(pool.clone(), pool.clone());
    let streamer = StreamerDbModel::new(
        "Recovery",
        "https://example.com/recovery",
        "platform-twitch",
    );
    streamers.create_streamer(&streamer).await.unwrap();
    let sessions = Arc::new(SqlxSessionRepository::new(pool.clone(), pool.clone()));
    let session = LiveSessionDbModel::new(&streamer.id);
    sessions.create_session(&session).await.unwrap();
    let dags = Arc::new(SqlxDagRepository::new(pool.clone(), pool.clone()));
    let definition = DagPipelineDefinition::new(
        "paired",
        vec![DagStep::new(
            "root",
            PipelineStep::inline("remux", serde_json::json!({})),
        )],
    );
    let statuses = ["PENDING", "PROCESSING", "COMPLETED", "FAILED", "CANCELLED"];
    // Cross the 500-row page boundary with every state represented on both pages.
    for index in 0..505 {
        let mut dag = DagExecutionDbModel::new(
            &definition,
            Some(streamer.id.clone()),
            Some(session.id.clone()),
        );
        dag.status = statuses[index % statuses.len()].to_owned();
        dag.segment_source = Some("paired".to_owned());
        dag.segment_index = Some(index as i64);
        dag.created_at = index as i64;
        dags.create_dag(&dag).await.unwrap();
    }
    let manager: PipelineManager = PipelineManager::with_repository(
        PipelineManagerConfig::default(),
        Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone())),
    )
    .with_session_repository(sessions)
    .with_dag_repository(dags);
    let trace = SqlTrace::install(&pool).await;
    manager.recover_pipeline_coordination().await.unwrap();
    let statements = trace.statements();
    let session_scans: Vec<_> = statements
        .iter()
        .filter(|sql| sql.starts_with("SELECT * FROM dag_execution WHERE session_id ="))
        .collect();
    assert_eq!(session_scans.len(), 2, "{statements:?}");
    assert_eq!(
        statements
            .iter()
            .filter(|sql| sql
                .starts_with("SELECT * FROM dag_execution WHERE status = ? AND session_id ="))
            .count(),
        0
    );
    assert_eq!(
        statements
            .iter()
            .filter(|sql| sql.starts_with("SELECT * FROM dag_execution WHERE status = ? ORDER BY"))
            .count(),
        2
    );
    pool.close().await;
}
