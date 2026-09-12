use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::{SqlitePool, migrate::Migrator, sqlite::SqlitePoolOptions};

use rust_srec::credentials::{
    CredentialScope, CredentialSource, CredentialStore, RefreshedCredentials,
};
use rust_srec::database::models::{
    DagPipelineDefinition, JobPreset, PipelinePreset, TemplateConfigDbModel,
};
use rust_srec::database::repositories::{
    ConfigRepository, JobPresetRepository, PipelinePresetRepository, SqliteJobPresetRepository,
    SqlitePipelinePresetRepository, SqlxConfigRepository, SqlxCredentialStore,
};

const VERSION: i64 = 20260907120000;
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");
const RETIRING_TABLES: [(&str, &str, &str, &str); 3] = [
    (
        "job_presets",
        "job_preset",
        ", processor, config",
        ", 'remux', '{}'",
    ),
    ("pipeline_presets", "pipeline_preset", "", ""),
    ("template_config", "template", "", ""),
];

async fn seed_retiring_configurations(pool: &SqlitePool) {
    for (table, kind, extra_columns, extra_values) in RETIRING_TABLES {
        for (id, timestamp) in [
            ("retiring-integer", "1788784496123"),
            ("retiring-text", "'2026-09-07T12:34:56.123456Z'"),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "INSERT INTO {table} (id, name, created_at, updated_at{extra_columns}) VALUES (?, ?, {timestamp}, {timestamp}{extra_values})"
            )))
            .bind(id).bind(id).execute(pool).await.unwrap();
            sqlx::query("INSERT INTO retirement_config_deletions (kind, config_id) VALUES (?, ?)")
                .bind(kind)
                .bind(id)
                .execute(pool)
                .await
                .unwrap();
        }
    }
    // Intentions without a current definition must also survive normalization.
    sqlx::query("INSERT INTO retirement_config_deletions (kind, config_id) VALUES ('template', 'missing-template')")
        .execute(pool).await.unwrap();
}

async fn retirement_deletions(pool: &SqlitePool) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT kind, config_id FROM retirement_config_deletions ORDER BY kind, config_id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn schema_definitions(pool: &SqlitePool) -> Vec<(String, String)> {
    sqlx::query_as("SELECT name, sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name")
        .fetch_all(pool)
        .await
        .unwrap()
}

// Schema equality belongs to this data-only migration, not later schema upgrades.
fn timestamp_migrator() -> Migrator {
    Migrator::with_migrations(
        MIGRATOR
            .iter()
            .filter(|migration| migration.version <= VERSION)
            .cloned()
            .collect::<Vec<_>>(),
    )
}

async fn previous_database() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    Migrator::with_migrations(
        MIGRATOR
            .iter()
            .filter(|migration| migration.version < VERSION)
            .cloned()
            .collect::<Vec<_>>(),
    )
    .run(&pool)
    .await
    .unwrap();
    pool
}

async fn timestamps(pool: &SqlitePool, table: &str, id: &str) -> (i64, i64) {
    let row: (i64, i64, String, String) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT created_at, updated_at, typeof(created_at), typeof(updated_at) FROM {table} WHERE id = ?"
    )))
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!((&*row.2, &*row.3), ("integer", "integer"));
    (row.0, row.1)
}

#[tokio::test]
async fn timestamp_migration_preserves_seeds_and_normalizes_historical_text() {
    let pool = previous_database().await;
    let seeds: Vec<(String, String, i64, i64)> =
        sqlx::query_as("SELECT 'job_presets', id, created_at, updated_at FROM job_presets UNION ALL SELECT 'pipeline_presets', id, created_at, updated_at FROM pipeline_presets UNION ALL SELECT 'template_config', id, created_at, updated_at FROM template_config")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(!seeds.is_empty());
    let schema: Vec<(String, String)> =
        sqlx::query_as("SELECT name, sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name")
            .fetch_all(&pool)
            .await
            .unwrap();

    let fixtures = [
        (
            "2026-09-07T12:34:56.123456789+02:30",
            "2026-09-07T10:04:56.123Z",
        ),
        (
            "2026-09-07 12:34:56.999999999+00:00",
            "2026-09-07T12:34:56.999Z",
        ),
        ("2026-09-07T12:34:56.1-04:00", "2026-09-07T16:34:56.100Z"),
        ("1969-12-31T23:59:59.999999Z", "1969-12-31T23:59:59.999Z"),
        ("2026-09-07 12:34:56", "2026-09-07T12:34:56Z"),
        ("2026-09-07T12:34:56.01Z", "2026-09-07T12:34:56.010Z"),
    ];
    for (index, (input, _)) in fixtures.iter().enumerate() {
        let id = format!("historical-{index}");
        for (table, extra_columns, extra_values) in [
            ("job_presets", ", processor, config", ", 'remux', '{}'"),
            ("pipeline_presets", "", ""),
            ("template_config", "", ""),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "INSERT INTO {table} (id, name, created_at, updated_at{extra_columns}) VALUES (?, ?, ?, ?{extra_values})"
            )))
            .bind(&id).bind(&id).bind(input).bind(input)
            .execute(&pool).await.unwrap();
        }
    }

    timestamp_migrator().run(&pool).await.unwrap();
    timestamp_migrator().run(&pool).await.unwrap();
    for (table, id, created, updated) in seeds {
        assert_eq!(timestamps(&pool, &table, &id).await, (created, updated));
    }
    for (index, (_, expected)) in fixtures.iter().enumerate() {
        let expected = DateTime::parse_from_rfc3339(expected)
            .unwrap()
            .timestamp_millis();
        for table in ["job_presets", "pipeline_presets", "template_config"] {
            assert_eq!(
                timestamps(&pool, table, &format!("historical-{index}")).await,
                (expected, expected)
            );
        }
    }
    let after: Vec<(String, String)> =
        sqlx::query_as("SELECT name, sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        schema, after,
        "indexes, triggers, and table definitions are unchanged"
    );
    // Keep the complete upgrade chain covered after checking the migration's own contract.
    MIGRATOR.run(&pool).await.unwrap();
    MIGRATOR.run(&pool).await.unwrap();
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "ok"
    );
    let jobs = sqlx::query_as::<_, JobPreset>("SELECT * FROM job_presets")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(!jobs.is_empty());
    assert_eq!(
        jobs.iter()
            .find(|job| job.id == "historical-0")
            .unwrap()
            .created_at
            .to_rfc3339(),
        "2026-09-07T10:04:56.123+00:00"
    );
}

#[tokio::test]
async fn timestamp_migration_rejects_malformed_values_atomically_and_can_retry() {
    for invalid in [
        "not-a-timestamp",
        "2026-02-30T12:00:00Z",
        "2026-09-07T12:00:00.Z",
        "+58000-01-01T00:00:00Z",
    ] {
        let pool = previous_database().await;
        sqlx::query("INSERT INTO template_config (id, name, created_at, updated_at) VALUES ('bad', 'bad', ?, '2026-09-07T12:34:56.123456Z')")
            .bind(invalid).execute(&pool).await.unwrap();
        let error = MIGRATOR.run(&pool).await.unwrap_err().to_string();
        assert!(
            error.contains("Cannot normalize preset/template timestamps"),
            "{error}"
        );
        let historical: (String, String) =
            sqlx::query_as("SELECT created_at, updated_at FROM template_config WHERE id = 'bad'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            historical,
            (invalid.to_owned(), "2026-09-07T12:34:56.123456Z".to_owned())
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _sqlx_migrations WHERE version = ?")
                .bind(VERSION)
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        sqlx::query("UPDATE template_config SET created_at = 0 WHERE id = 'bad'")
            .execute(&pool)
            .await
            .unwrap();
        MIGRATOR.run(&pool).await.unwrap();
        assert_eq!(timestamps(&pool, "template_config", "bad").await.0, 0);
    }
}

#[tokio::test]
async fn timestamp_migration_preserves_retirement_intentions_and_user_edit_triggers() {
    let pool = previous_database().await;
    seed_retiring_configurations(&pool).await;
    let mut pending = retirement_deletions(&pool).await;
    assert_eq!(pending.len(), 7);
    let schema = schema_definitions(&pool).await;

    timestamp_migrator().run(&pool).await.unwrap();
    timestamp_migrator().run(&pool).await.unwrap();
    assert_eq!(retirement_deletions(&pool).await, pending);
    assert_eq!(schema_definitions(&pool).await, schema);
    // Keep the complete upgrade chain covered after checking the migration's own contract.
    MIGRATOR.run(&pool).await.unwrap();
    MIGRATOR.run(&pool).await.unwrap();

    for (table, kind, _, _) in RETIRING_TABLES {
        for id in ["retiring-integer", "retiring-text"] {
            assert_eq!(
                timestamps(&pool, table, id).await,
                (1788784496123, 1788784496123)
            );
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE {table} SET name = ? WHERE id = ?"
            )))
            .bind(format!("{id}-kept"))
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
            pending.retain(|(pending_kind, pending_id)| pending_kind != kind || pending_id != id);
            assert_eq!(retirement_deletions(&pool).await, pending);
        }
    }
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "ok"
    );
}

#[tokio::test]
async fn timestamp_migration_rolls_back_retirement_cancellations_and_can_retry() {
    let pool = previous_database().await;
    seed_retiring_configurations(&pool).await;
    let pending = retirement_deletions(&pool).await;
    let schema = schema_definitions(&pool).await;
    // Fail after the job and pipeline updates have already canceled intentions.
    sqlx::raw_sql(
        "CREATE TRIGGER fail_timestamp_normalization BEFORE UPDATE ON template_config
         BEGIN SELECT RAISE(ABORT, 'injected normalization failure'); END;",
    )
    .execute(&pool)
    .await
    .unwrap();

    let error = timestamp_migrator()
        .run(&pool)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("injected normalization failure"), "{error}");
    assert_eq!(retirement_deletions(&pool).await, pending);
    for (table, _, _, _) in RETIRING_TABLES {
        assert_eq!(
            timestamps(&pool, table, "retiring-integer").await,
            (1788784496123, 1788784496123)
        );
        let text: (String, String) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT created_at, updated_at FROM {table} WHERE id = 'retiring-text'"
        )))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(text.0, "2026-09-07T12:34:56.123456Z");
        assert_eq!(text.1, text.0);
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _sqlx_migrations WHERE version = ?")
            .bind(VERSION)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    sqlx::query("DROP TRIGGER fail_timestamp_normalization")
        .execute(&pool)
        .await
        .unwrap();
    timestamp_migrator().run(&pool).await.unwrap();
    assert_eq!(retirement_deletions(&pool).await, pending);
    assert_eq!(schema_definitions(&pool).await, schema);
    // Keep the complete upgrade chain covered after checking the migration's own contract.
    MIGRATOR.run(&pool).await.unwrap();
    MIGRATOR.run(&pool).await.unwrap();
    for (table, _, _, _) in RETIRING_TABLES {
        for id in ["retiring-integer", "retiring-text"] {
            assert_eq!(
                timestamps(&pool, table, id).await,
                (1788784496123, 1788784496123)
            );
        }
    }
}

#[tokio::test]
async fn timestamp_repository_writes_and_credentials_keep_integer_storage_and_string_contracts() {
    let pool = previous_database().await;
    MIGRATOR.run(&pool).await.unwrap();
    let job_repo = SqliteJobPresetRepository::new(Arc::new(pool.clone()), Arc::new(pool.clone()));
    let pipeline_repo =
        SqlitePipelinePresetRepository::new(Arc::new(pool.clone()), Arc::new(pool.clone()));
    let config_repo = SqlxConfigRepository::new(pool.clone(), pool.clone());
    let at = DateTime::parse_from_rfc3339("2026-01-02T03:04:05.123456Z")
        .unwrap()
        .with_timezone(&Utc);
    let expected = at.timestamp_millis();
    let mut job = JobPreset::new("timestamp-job", "remux", serde_json::json!({}));
    job.created_at = at;
    job.updated_at = at;
    let mut pipeline = PipelinePreset::new(
        "timestamp-pipeline",
        DagPipelineDefinition::new("test", vec![]),
    );
    pipeline.created_at = at;
    pipeline.updated_at = at;
    let mut template = TemplateConfigDbModel::new("timestamp-template");
    template.created_at = at;
    template.updated_at = at;
    job_repo.create_preset(&job).await.unwrap();
    pipeline_repo
        .create_pipeline_preset(&pipeline)
        .await
        .unwrap();
    config_repo.create_template_config(&template).await.unwrap();
    let job_read = job_repo.get_preset(&job.id).await.unwrap().unwrap();
    let pipeline_read = pipeline_repo
        .get_pipeline_preset(&pipeline.id)
        .await
        .unwrap()
        .unwrap();
    let template_read = config_repo.get_template_config(&template.id).await.unwrap();
    for value in [
        serde_json::to_value(&job_read).unwrap(),
        serde_json::to_value(&pipeline_read).unwrap(),
        serde_json::to_value(&template_read).unwrap(),
    ] {
        assert_eq!(value["created_at"], "2026-01-02T03:04:05.123Z");
        assert_eq!(value["updated_at"], "2026-01-02T03:04:05.123Z");
    }
    for (table, id) in [
        ("job_presets", &job.id),
        ("pipeline_presets", &pipeline.id),
        ("template_config", &template.id),
    ] {
        assert_eq!(timestamps(&pool, table, id).await, (expected, expected));
    }
    job_repo.update_preset(&job).await.unwrap();
    pipeline_repo
        .update_pipeline_preset(&pipeline)
        .await
        .unwrap();
    config_repo.update_template_config(&template).await.unwrap();
    for (table, id) in [
        ("job_presets", &job.id),
        ("pipeline_presets", &pipeline.id),
        ("template_config", &template.id),
    ] {
        let (created, updated) = timestamps(&pool, table, id).await;
        assert_eq!(created, expected);
        assert!(updated >= expected);
    }
    let store = SqlxCredentialStore::new(pool.clone(), pool.clone());
    let mut source = CredentialSource {
        scope: CredentialScope::Template {
            template_id: template.id.clone(),
            template_name: template.name.clone(),
        },
        cookies: String::new(),
        refresh_token: None,
        access_token: None,
        platform_name: "bilibili".to_owned(),
        reauth_extra: None,
    };
    for refresh_token in [None, Some("test-refresh".to_owned())] {
        store
            .update_credentials(
                &source,
                &RefreshedCredentials {
                    cookies: "test-cookie=value".to_owned(),
                    refresh_token,
                    access_token: None,
                    expires_at: None,
                },
            )
            .await
            .unwrap();
        source = store.reload_source(&source).await.unwrap();
        let refreshed = config_repo.get_template_config(&template.id).await.unwrap();
        assert_eq!(refreshed.created_at.timestamp_millis(), expected);
        assert_eq!(refreshed.cookies.as_deref(), Some("test-cookie=value"));
        assert_eq!(
            refreshed.updated_at.timestamp_millis(),
            timestamps(&pool, "template_config", &template.id).await.1
        );
    }

    sqlx::query("UPDATE template_config SET updated_at = ? WHERE id = ?")
        .bind(i64::MAX)
        .bind(&template.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(config_repo.get_template_config(&template.id).await.is_err());
    sqlx::query("UPDATE template_config SET updated_at = '2026-01-02T03:04:05Z' WHERE id = ?")
        .bind(&template.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(config_repo.get_template_config(&template.id).await.is_err());
}
