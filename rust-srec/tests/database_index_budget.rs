use rust_srec::database::{
    self,
    models::{OutputFilters, Pagination},
    repositories::{
        DagRepository, JobRepository, SessionRepository, SqlxDagRepository, SqlxJobRepository,
        SqlxSessionRepository,
    },
};
use sqlx::{Row, SqlitePool, migrate::Migrator};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");
const INDEX_MIGRATION: i64 = 20260908130000;
const DAG_PAGE: &str = "SELECT * FROM dag_execution ORDER BY created_at DESC LIMIT ? OFFSET ?";
const MEDIA_PAGE: &str = "SELECT m.id,m.session_id,m.parent_media_output_id,m.file_path,m.file_type,m.size_bytes,m.created_at FROM media_outputs m ORDER BY m.created_at DESC LIMIT ? OFFSET ?";
// The bind shape is important: SQLite cannot use a partial index whose literal
// status predicate is not implied by these parameters, even with current stats.
const CLEANUP_PAGE: &str = "SELECT j.id FROM job j WHERE j.status IN (?, ?, ?) AND j.updated_at < ? AND NOT EXISTS (SELECT 1 FROM dag_step_execution s JOIN dag_execution d ON d.id = s.dag_id WHERE s.job_id = j.id AND (d.status NOT IN (?, ?, ?) OR d.updated_at >= ?)) ORDER BY j.updated_at ASC LIMIT ?";
const AVERAGE: &str = "SELECT AVG(COALESCE(duration_secs, CASE WHEN started_at IS NOT NULL AND completed_at IS NOT NULL THEN (completed_at-started_at)/1000.0 ELSE NULL END)) FROM job WHERE status = ? AND (duration_secs IS NOT NULL OR (started_at IS NOT NULL AND completed_at IS NOT NULL))";

async fn page_plan(pool: &SqlitePool, query: &str) -> String {
    sqlx::query(sqlx::AssertSqlSafe(format!("EXPLAIN QUERY PLAN {query}")))
        .bind(50)
        .bind(0)
        .fetch_all(pool)
        .await
        .unwrap()
        .iter()
        .map(|row| row.get::<String, _>("detail"))
        .collect::<Vec<_>>()
        .join("\n")
}

async fn cleanup_rows(pool: &SqlitePool, explain: bool) -> Vec<String> {
    let query = if explain {
        format!("EXPLAIN QUERY PLAN {CLEANUP_PAGE}")
    } else {
        CLEANUP_PAGE.to_owned()
    };
    sqlx::query(sqlx::AssertSqlSafe(query))
        .bind("COMPLETED")
        .bind("FAILED")
        .bind("CANCELLED")
        .bind(2_500_000)
        .bind("COMPLETED")
        .bind("FAILED")
        .bind("CANCELLED")
        .bind(2_500_000)
        .bind(200)
        .fetch_all(pool)
        .await
        .unwrap()
        .iter()
        .map(|row| row.get::<String, _>(if explain { "detail" } else { "id" }))
        .collect()
}

#[tokio::test]
async fn index_upgrade_removes_page_sorts_and_preserves_cleanup_and_statistics() {
    let pool = database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    Migrator::with_migrations(
        MIGRATOR
            .iter()
            .filter(|migration| migration.version < INDEX_MIGRATION)
            .cloned()
            .collect::<Vec<_>>(),
    )
    .run(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!("fixtures/database_query_plans.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("ANALYZE").execute(&pool).await.unwrap();
    assert!(
        page_plan(&pool, DAG_PAGE)
            .await
            .contains("USE TEMP B-TREE FOR ORDER BY")
    );
    assert!(
        page_plan(&pool, MEDIA_PAGE)
            .await
            .contains("USE TEMP B-TREE FOR ORDER BY")
    );
    let before = cleanup_rows(&pool, false).await;
    assert!(
        cleanup_rows(&pool, true)
            .await
            .join("\n")
            .contains("idx_job_updated_at")
    );

    MIGRATOR.run(&pool).await.unwrap();
    MIGRATOR.run(&pool).await.unwrap();
    sqlx::query("ANALYZE").execute(&pool).await.unwrap();
    for (query, index) in [
        (DAG_PAGE, "idx_dag_execution_created_at"),
        (MEDIA_PAGE, "idx_media_outputs_created_at"),
    ] {
        let plan = page_plan(&pool, query).await;
        assert!(plan.contains(index), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    }
    for index in [
        "idx_job_started_at",
        "idx_job_completed_at",
        "idx_jobs_completed_at_status",
        "idx_job_terminal_updated_at",
    ] {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_schema WHERE type='index' AND name=?")
                .bind(index)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 0, "{index}");
    }
    assert_eq!(cleanup_rows(&pool, false).await, before);
    assert!(
        cleanup_rows(&pool, true)
            .await
            .join("\n")
            .contains("idx_job_updated_at")
    );
    let average_plan = sqlx::query(sqlx::AssertSqlSafe(format!("EXPLAIN QUERY PLAN {AVERAGE}")))
        .bind("COMPLETED")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(average_plan.iter().any(|row| {
        row.get::<String, _>("detail")
            .contains("COVERING INDEX idx_job_status_duration")
    }));

    // Exercise the actual consumers as well as their plans so projection/binding drift fails.
    let dags = SqlxDagRepository::new(pool.clone(), pool.clone())
        .list_dags(None, None, 50, 0)
        .await
        .unwrap();
    assert_eq!(dags.first().unwrap().id, "plan-dag-5000");
    assert_eq!(dags.last().unwrap().id, "plan-dag-4951");
    let (outputs, total) = SqlxSessionRepository::new(pool.clone(), pool.clone())
        .list_outputs_filtered(
            &OutputFilters::default(),
            &Pagination {
                limit: 50,
                offset: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(total, 5000);
    assert_eq!(outputs.first().unwrap().id, "plan-media-5000");
    assert_eq!(outputs.last().unwrap().id, "plan-media-4951");
    assert_eq!(
        SqlxJobRepository::new(pool.clone(), pool.clone())
            .get_avg_processing_time()
            .await
            .unwrap(),
        Some(1.0)
    );
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(integrity, "ok");
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap()
            .is_empty()
    );
    pool.close().await;
}
