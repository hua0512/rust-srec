//! Repository layer for database access.
//!
//! This module implements the Repository Pattern to abstract all database interactions,
//! creating a clean and maintainable data access layer.

pub mod row_write;

pub mod config;
pub mod config_retirement;
pub mod credential_profiles;
pub mod credential_selections;
pub mod filter;
pub mod monitor_outbox;
pub mod notification;
pub mod preset;
pub mod proxies;
pub mod session;
pub mod session_event;
pub mod session_tx;
pub mod streamer;
pub mod streamer_check_history;
pub mod streamer_tx;
pub mod tool_credential;

#[cfg(test)]
mod write_contract_tests;

pub use config::*;
pub use credential_profiles::*;
pub use credential_selections::StoredSelection;
pub use filter::*;
pub use monitor_outbox::*;
pub use notification::*;
pub use preset::*;
pub use session::*;
pub use session_event::*;
pub use session_tx::*;
pub use streamer::*;
pub use streamer_check_history::*;
pub use streamer_tx::*;
pub use tool_credential::*;

/// Bind a literal substring to `LIKE ... ESCAPE '\'` without changing SQLite's
/// case matching. The surrounding percent signs are the only wildcards.
pub fn literal_substring_pattern(search: &str) -> String {
    let mut pattern = String::with_capacity(search.len() + 2);
    pattern.push('%');
    for character in search.chars() {
        if matches!(character, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern.push('%');
    pattern
}

/// Remove `key` from a stored JSON-object document, rewriting `raw` only when
/// the key was present. Text that is not a JSON object is left as written.
pub fn take_key(raw: &mut Option<String>, key: &str) -> Option<serde_json::Value> {
    let mut document: serde_json::Value = serde_json::from_str(raw.as_deref()?).ok()?;
    let value = document.as_object_mut()?.remove(key)?;
    *raw = Some(document.to_string());
    Some(value)
}

/// Replace `key` in a JSON-object document for a reader: any value the
/// document carries is dropped, and `value`, when present, is inserted. A
/// missing or null document becomes an object only when there is a value.
pub fn inject_key(
    document: Option<serde_json::Value>,
    key: &str,
    value: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    let mut document = document.filter(|document| !document.is_null());
    if let Some(fields) = document.as_mut().and_then(serde_json::Value::as_object_mut) {
        fields.remove(key);
    }
    let Some(value) = value else {
        return document;
    };
    let mut document = document.unwrap_or_else(|| serde_json::Value::Object(Default::default()));
    if let Some(fields) = document.as_object_mut() {
        fields.insert(key.to_owned(), value);
    }
    Some(document)
}

/// SQLite's historical 999-parameter bind limit.
pub const MAX_SQLITE_BIND_VARS: usize = 999;

/// Stay below [`MAX_SQLITE_BIND_VARS`] while bounding each lookup.
pub const LOOKUP_BATCH_SIZE: usize = 500;

/// Whether `error` is SQLite's `SQLITE_CONSTRAINT_UNIQUE` (`2067`).
///
/// Primary-key collisions (`SQLITE_CONSTRAINT_PRIMARYKEY`, `1555`) are excluded
/// so callers mapping unique-column conflicts (such as preset `name`) do not
/// misreport an ID collision.
pub fn is_sqlite_unique_violation(error: &sqlx::Error) -> bool {
    matches!(
        error,
        sqlx::Error::Database(db_err) if db_err.code().as_deref() == Some("2067")
    )
}

pub fn unique_lookup_ids(ids: &[String]) -> Vec<&str> {
    let mut seen = std::collections::HashSet::new();
    ids.iter()
        .map(String::as_str)
        .filter(|id| seen.insert(*id))
        .collect()
}

/// Run `prefix (?, ?, ...) suffix` once per [`LOOKUP_BATCH_SIZE`] chunk of
/// `ids` and concatenate the rows. An `ORDER BY` in `suffix` orders each chunk,
/// not the whole result.
pub async fn fetch_in_chunks<T>(
    pool: &sqlx::SqlitePool,
    prefix: &'static str,
    ids: &[&str],
    suffix: &'static str,
) -> crate::DbResult<Vec<T>>
where
    T: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin,
{
    let mut rows = Vec::new();
    for ids in ids.chunks(LOOKUP_BATCH_SIZE) {
        let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(prefix);
        let mut separated = query.separated(", ");
        for id in ids {
            separated.push_bind(*id);
        }
        separated.push_unseparated(suffix);
        rows.extend(query.build_query_as::<T>().fetch_all(pool).await?);
    }
    Ok(rows)
}
