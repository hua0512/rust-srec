//! Repository layer for database access.
//!
//! This module implements the Repository Pattern to abstract all database interactions,
//! creating a clean and maintainable data access layer.

pub(crate) mod row_write;

pub mod api_key;
pub mod config;
pub(crate) mod config_retirement;
pub mod credential_profiles;
pub mod credential_selections;
pub mod dag;
pub mod filter;
pub mod job;
pub mod monitor_outbox;
pub mod notification;
pub mod preset;
pub mod proxies;
pub mod refresh_token;
pub mod session;
pub mod session_event;
pub mod session_lifecycle;
pub mod session_tx;
pub mod staged_output;
pub mod streamer;
pub mod streamer_check_history;
pub mod streamer_tx;
pub mod tool_credential;
pub mod upload_record;
pub mod user;

#[cfg(test)]
mod write_contract_tests;

pub use api_key::*;
pub use config::*;
pub use credential_profiles::*;
pub use credential_selections::StoredSelection;
pub use dag::*;
pub use filter::*;
pub use job::*;
pub use monitor_outbox::*;
pub use notification::*;
pub use preset::*;
pub use refresh_token::*;
pub use session::*;
pub use session_event::*;
pub use session_lifecycle::*;
pub use session_tx::*;
pub use staged_output::*;
pub use streamer::*;
pub use streamer_check_history::*;
pub use streamer_tx::*;
pub use tool_credential::*;
pub use upload_record::*;
pub use user::*;

/// Bind a literal substring to `LIKE ... ESCAPE '\'` without changing SQLite's
/// case matching. The surrounding percent signs are the only wildcards.
fn literal_substring_pattern(search: &str) -> String {
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
pub(crate) fn take_key(raw: &mut Option<String>, key: &str) -> Option<serde_json::Value> {
    let mut document: serde_json::Value = serde_json::from_str(raw.as_deref()?).ok()?;
    let value = document.as_object_mut()?.remove(key)?;
    *raw = Some(document.to_string());
    Some(value)
}

/// Replace `key` in a JSON-object document for a reader: any value the
/// document carries is dropped, and `value`, when present, is inserted. A
/// missing or null document becomes an object only when there is a value.
pub(crate) fn inject_key(
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

/// Stay below SQLite's historical 999-parameter limit while bounding each lookup.
pub(crate) const LOOKUP_BATCH_SIZE: usize = 500;

pub(crate) fn unique_lookup_ids(ids: &[String]) -> Vec<&str> {
    let mut seen = std::collections::HashSet::new();
    ids.iter()
        .map(String::as_str)
        .filter(|id| seen.insert(*id))
        .collect()
}

/// Run `prefix (?, ?, ...) suffix` once per [`LOOKUP_BATCH_SIZE`] chunk of
/// `ids` and concatenate the rows. An `ORDER BY` in `suffix` orders each chunk,
/// not the whole result.
pub(crate) async fn fetch_in_chunks<T>(
    pool: &sqlx::SqlitePool,
    prefix: &'static str,
    ids: &[&str],
    suffix: &'static str,
) -> crate::Result<Vec<T>>
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

#[cfg(test)]
mod import_projection_tests;
