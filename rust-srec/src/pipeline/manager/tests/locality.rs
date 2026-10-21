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
