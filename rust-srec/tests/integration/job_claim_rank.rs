use rust_srec::database::{
    self,
    repositories::{JobRepository, SqlxJobRepository},
};
use sqlx::{SqlitePool, migrate::Migrator};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");
const RANK_MIGRATION: i64 = 20260926000000;

async fn legacy_candidates(pool: &SqlitePool, types: &[String]) -> Vec<String> {
    let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT job.id FROM job LEFT JOIN dag_step_execution step ON step.id = job.dag_step_execution_id \
         WHERE job.status = 'PENDING' AND NOT EXISTS (SELECT 1 FROM dag_step_execution owner_step \
         JOIN dag_execution owner_dag ON owner_dag.id = owner_step.dag_id \
         WHERE owner_step.id = job.dag_step_execution_id AND (owner_step.status IN ('COMPLETED','FAILED','CANCELLED') \
         OR owner_dag.status IN ('COMPLETED','FAILED','CANCELLED')))",
    );
    if !types.is_empty() {
        query.push(" AND job.job_type IN (");
        let mut separated = query.separated(",");
        for kind in types {
            separated.push_bind(kind);
        }
        separated.push_unseparated(")");
    }
    query.push(
        " ORDER BY job.priority DESC, \
         CASE WHEN step.depends_on_step_ids IS NOT NULL AND step.depends_on_step_ids != '[]' \
              THEN 0 ELSE 1 END, job.created_at, job.id",
    );
    query.build_query_scalar().fetch_all(pool).await.unwrap()
}

async fn rank(pool: &SqlitePool, id: &str) -> i64 {
    sqlx::query_scalar("SELECT continuation_rank FROM job WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn rank_upgrade_preserves_claim_order_and_tracks_relinks_and_dependencies() {
    let pool = database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    Migrator::with_migrations(
        MIGRATOR
            .iter()
            .filter(|m| m.version < RANK_MIGRATION)
            .cloned()
            .collect::<Vec<_>>(),
    )
    .run(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(r#"
        INSERT INTO dag_execution(id,dag_definition,status,created_at,updated_at,total_steps)
        VALUES ('live','{}','PROCESSING',1,1,3), ('dead','{}','CANCELLED',1,1,1);
        INSERT INTO dag_step_execution(id,dag_id,step_id,status,depends_on_step_ids,created_at,updated_at)
        VALUES ('root','live','root','PENDING','[]',1,1),
               ('next','live','next','PENDING','["root"]',1,1),
               ('settled','live','settled','COMPLETED','[]',1,1),
               ('dead-step','dead','dead-step','PENDING','["root"]',1,1);
        INSERT INTO job(id,job_type,status,config,state,priority,created_at,updated_at,dag_step_execution_id)
        VALUES ('urgent','remux','PENDING','{}','{}',5,100,7,'root'),
               ('next-new','remux','PENDING','{}','{}',0,30,7,'next'),
               ('next-b','delete','PENDING','{}','{}',0,20,7,'next'),
               ('next-a','remux','PENDING','{}','{}',0,20,7,'next'),
               ('plain','delete','PENDING','{}','{}',0,1,7,NULL),
               ('root-job','remux','PENDING','{}','{}',0,2,7,'root'),
               ('step-ended','remux','PENDING','{}','{}',100,0,7,'settled'),
               ('dag-ended','remux','PENDING','{}','{}',100,0,7,'dead-step'),
               ('done','remux','COMPLETED','{}','{}',100,0,7,'root');
    "#).execute(&pool).await.unwrap();
    let filters = [
        vec![],
        vec!["remux".to_string()],
        vec!["delete".to_string(), "remux".to_string()],
        vec!["missing".to_string()],
    ];
    let mut expected = Vec::new();
    for types in &filters {
        expected.push(legacy_candidates(&pool, types).await);
    }
    assert_eq!(
        expected[0],
        [
            "urgent", "next-a", "next-b", "next-new", "plain", "root-job"
        ]
    );
    MIGRATOR.run(&pool).await.unwrap();
    MIGRATOR.run(&pool).await.unwrap();
    let unchanged: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM job WHERE updated_at=7")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(unchanged, 9);
    let repo = SqlxJobRepository::new(pool.clone(), pool.clone());
    for (types, expected) in filters.iter().zip(expected) {
        sqlx::query(
            "UPDATE job SET status = CASE WHEN id='done' THEN 'COMPLETED' ELSE 'PENDING' END",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut actual = Vec::new();
        while let Some(job) = repo.claim_next_pending_job(Some(types)).await.unwrap() {
            actual.push(job.id);
        }
        assert_eq!(actual, expected);
    }
    assert_eq!(rank(&pool, "plain").await, 1);
    sqlx::query("UPDATE job SET dag_step_execution_id='next' WHERE id='plain'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(rank(&pool, "plain").await, 0);
    sqlx::query("UPDATE job SET dag_step_execution_id=NULL WHERE id='plain'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(rank(&pool, "plain").await, 1);
    sqlx::query("UPDATE dag_step_execution SET depends_on_step_ids='[]' WHERE id='next'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(rank(&pool, "next-a").await, 1);
    sqlx::query("UPDATE dag_step_execution SET depends_on_step_ids='[\"root\"]' WHERE id='next'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(rank(&pool, "next-a").await, 0);
    sqlx::query("UPDATE job SET status='FAILED' WHERE id='next-a'")
        .execute(&pool)
        .await
        .unwrap();
    repo.reset_job_for_retry("next-a").await.unwrap();
    assert_eq!(rank(&pool, "next-a").await, 0);

    // Deferred publication must derive the rank even when the step is inserted last.
    let mut tx = pool.begin().await.unwrap();
    sqlx::raw_sql(r#"
        PRAGMA defer_foreign_keys=ON;
        INSERT INTO job(id,job_type,status,config,state,created_at,updated_at,dag_step_execution_id)
        VALUES ('deferred','remux','PENDING','{}','{}',0,0,'later');
        INSERT INTO dag_step_execution(id,dag_id,step_id,status,depends_on_step_ids,created_at,updated_at)
        VALUES ('later','live','later','PENDING','["root"]',0,0);
    "#).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(rank(&pool, "deferred").await, 0);
    sqlx::query("DELETE FROM dag_step_execution WHERE id='later'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(rank(&pool, "deferred").await, 1);
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

#[tokio::test]
async fn pending_rank_indexes_avoid_sorting_single_and_mixed_job_types() {
    use sqlx::Row;

    let pool = database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    MIGRATOR.run(&pool).await.unwrap();
    sqlx::raw_sql(
        r#"
        WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<5000)
        INSERT INTO job(id,job_type,status,config,state,priority,created_at,updated_at)
        SELECT 'job-'||i, CASE WHEN i%2=0 THEN 'remux' ELSE 'delete' END,
               'PENDING','{}','{}',i%3,i,i FROM n;
        ANALYZE;
    "#,
    )
    .execute(&pool)
    .await
    .unwrap();
    for (predicate, index) in [
        ("", "idx_job_pending_priority_created_at"),
        (
            " AND job.job_type IN ('remux')",
            "idx_job_pending_type_priority_created_at",
        ),
        (
            " AND job.job_type IN ('remux','delete')",
            "idx_job_pending_priority_created_at",
        ),
    ] {
        let query = format!(
            "EXPLAIN QUERY PLAN SELECT job.id FROM job WHERE job.status = 'PENDING' {predicate} \
             AND NOT EXISTS (SELECT 1 FROM dag_step_execution owner_step \
             JOIN dag_execution owner_dag ON owner_dag.id = owner_step.dag_id \
             WHERE owner_step.id = job.dag_step_execution_id \
             AND (owner_step.status IN ('COMPLETED','FAILED','CANCELLED') \
                  OR owner_dag.status IN ('COMPLETED','FAILED','CANCELLED'))) \
             ORDER BY job.priority DESC, job.continuation_rank, job.created_at, job.id LIMIT 1"
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(query))
            .fetch_all(&pool)
            .await
            .unwrap();
        let plan = rows
            .iter()
            .map(|row| row.get::<String, _>("detail"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(plan.contains(index), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    }
    pool.close().await;
}
