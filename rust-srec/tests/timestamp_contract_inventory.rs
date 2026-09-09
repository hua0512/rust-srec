//! Semantic inventory of canonical SQLite clocks. JSON payload timestamps and
//! durations are not columns in this contract. New columns require classification,
//! regardless of their spelling (including future `*_at_ms` columns).

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use sqlx::{Row, SqlitePool, sqlite::SqlitePoolOptions};

#[derive(Deserialize)]
struct TableContract {
    required_epoch_ms: Vec<String>,
    nullable_epoch_ms: Vec<String>,
    non_timestamp_columns: Vec<String>,
}

#[tokio::test]
async fn canonical_timestamp_inventory_storage_and_nullability() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    let inventory: BTreeMap<String, TableContract> =
        serde_json::from_str(include_str!("fixtures/timestamp_contract_inventory.json")).unwrap();
    let tables: BTreeSet<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations'",
    ).fetch_all(&pool).await.unwrap().into_iter().collect();
    assert_eq!(tables, inventory.keys().cloned().collect());

    // These are storage fixtures, not proof of application write bindings. Public
    // account/token writes are checked separately in account_token_contracts;
    // preset_template_timestamps covers the DateTime/EpochMillis adapters.
    sqlx::raw_sql(include_str!("fixtures/timestamp_contract_rows.sql"))
        .execute(&pool)
        .await
        .unwrap();
    for (table, contract) in &inventory {
        let columns = sqlx::query("SELECT name, type, [notnull] FROM pragma_table_info(?)")
            .bind(table)
            .fetch_all(&pool)
            .await
            .unwrap();
        let classified: Vec<&String> = contract
            .required_epoch_ms
            .iter()
            .chain(&contract.nullable_epoch_ms)
            .chain(&contract.non_timestamp_columns)
            .collect();
        let expected: BTreeSet<&str> = classified.iter().map(|column| column.as_str()).collect();
        assert_eq!(
            expected.len(),
            classified.len(),
            "duplicate classification: {table}"
        );
        let actual: BTreeSet<&str> = columns.iter().map(|column| column.get("name")).collect();
        assert_eq!(actual, expected, "classify every column in {table}");
        for (nullable, timestamps) in [
            (false, &contract.required_epoch_ms),
            (true, &contract.nullable_epoch_ms),
        ] {
            for column in timestamps {
                let metadata = columns
                    .iter()
                    .find(|row| row.get::<&str, _>("name") == column)
                    .unwrap();
                assert_eq!(
                    metadata.get::<&str, _>("type"),
                    "INTEGER",
                    "{table}.{column}"
                );
                assert_eq!(
                    metadata.get::<i64, _>("notnull"),
                    i64::from(!nullable),
                    "{table}.{column}"
                );
                assert_populated_integer(&pool, table, column, nullable).await;
                let reset = sqlx::query(sqlx::AssertSqlSafe(format!(
                    "UPDATE {table} SET {column} = NULL"
                )))
                .execute(&pool)
                .await;
                if nullable {
                    assert!(reset.unwrap().rows_affected() > 0, "{table}.{column}");
                    let nonnull: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                        "SELECT COUNT(*) FROM {table} WHERE {column} IS NOT NULL"
                    )))
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                    assert_eq!(nonnull, 0, "nullable reset: {table}.{column}");
                } else {
                    let error = reset.unwrap_err();
                    assert_eq!(
                        error.as_database_error().unwrap().kind(),
                        sqlx::error::ErrorKind::NotNullViolation,
                        "{table}.{column}: {error}"
                    );
                }
            }
        }
    }
    let violations = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(violations.is_empty());
}

async fn assert_populated_integer(pool: &SqlitePool, table: &str, column: &str, nullable: bool) {
    // Inspect storage before null-reset probes; never rewrite a clock to make its
    // type pass. Check all seeded/default rows plus the exact millisecond fixture.
    let values: Vec<(String, Option<i64>)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT typeof({column}), {column} FROM {table}"
    )))
    .fetch_all(pool)
    .await
    .unwrap();
    assert!(!values.is_empty(), "no fixture for {table}.{column}");
    assert!(
        values
            .iter()
            .any(|(_, value)| *value == Some(1_788_784_496_123)),
        "no populated clock fixture: {table}.{column}"
    );
    for (storage, value) in values {
        assert_eq!(
            storage,
            if nullable && value.is_none() {
                "null"
            } else {
                "integer"
            },
            "{table}.{column}: {value:?}"
        );
    }
}
