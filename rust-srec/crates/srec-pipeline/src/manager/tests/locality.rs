use super::*;

use crate::repositories::{SqlxDagRepository, SqlxJobRepository};
use srec_sqlite::models::StreamerDbModel;
use srec_sqlite::repositories::SqlxSessionRepository;
use srec_sqlite::test_support::SqlTrace;

async fn database() -> sqlx::SqlitePool {
    let pool = srec_sqlite::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    srec_sqlite::run_migrations(&pool).await.unwrap();
    pool
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
    let service = Arc::new(TestConfigService::new(configs, streamers.clone()));
    let jobs = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
    let (sess_port, cfg_port) = wired_ports(sessions, streamers, service);
    let manager = PipelineManager::with_repository(PipelineManagerConfig::default(), jobs.clone())
        .with_session_port(sess_port)
        .with_config_port(cfg_port)
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

/// Coordination recovery pages through every DAG of a session, across the
/// 500-row page boundary, in one scan per page rather than one per status;
/// stopping after the first page would leave later segments' DAGs untracked.
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
    let template = DagExecutionDbModel::new(
        &definition,
        Some(streamer.id.clone()),
        Some(session.id.clone()),
    );
    // Cross the 500-row page boundary with every state represented on both pages.
    sqlx::query(
        "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < 504) \
         INSERT INTO dag_execution (id, dag_definition, status, streamer_id, session_id, \
             created_at, updated_at, total_steps, completed_steps, failed_steps, \
             segment_source, segment_index) \
         SELECT 'paired-' || i, ?, \
             CASE i % 5 WHEN 0 THEN 'PENDING' WHEN 1 THEN 'PROCESSING' \
                 WHEN 2 THEN 'COMPLETED' WHEN 3 THEN 'FAILED' ELSE 'CANCELLED' END, \
             ?, ?, i, i, ?, 0, 0, 'paired', i \
         FROM n",
    )
    .bind(&template.dag_definition)
    .bind(&template.streamer_id)
    .bind(&template.session_id)
    .bind(template.total_steps)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(dags.count_dags(None, Some(&session.id)).await.unwrap(), 505);
    let manager = PipelineManager::with_repository(
        PipelineManagerConfig::default(),
        Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone())),
    )
    .with_session_port(session_port(sessions))
    .with_dag_repository(dags);
    let trace = SqlTrace::install(&pool).await;
    manager.recover_pipeline_coordination().await.unwrap();
    let statements = trace.statements();
    let count = |prefix: &str| {
        statements
            .iter()
            .filter(|sql| sql.starts_with(prefix))
            .count()
    };
    assert_eq!(
        count("SELECT * FROM dag_execution WHERE session_id ="),
        2,
        "{statements:?}"
    );
    assert_eq!(
        count("SELECT * FROM dag_execution WHERE status = ? AND session_id ="),
        0
    );
    assert_eq!(
        count("SELECT * FROM dag_execution WHERE status = ? ORDER BY"),
        2
    );
    pool.close().await;
}
