//! Repository layer for database access.
//!
//! This module implements the Repository Pattern to abstract all database interactions,
//! creating a clean and maintainable data access layer.

pub mod api_key;
pub mod config;
pub(crate) mod config_retirement;
pub mod credential_store;
pub mod dag;
pub mod filter;
pub mod job;
pub mod monitor_outbox;
pub mod notification;
pub mod preset;
pub mod refresh_token;
pub mod session;
pub mod session_event;
pub mod session_lifecycle;
pub mod session_tx;
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
pub use credential_store::*;
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

/// Stay below SQLite's historical 999-parameter limit while bounding each lookup.
pub(crate) const LOOKUP_BATCH_SIZE: usize = 500;

pub(crate) fn unique_lookup_ids(ids: &[String]) -> Vec<&str> {
    let mut seen = std::collections::HashSet::new();
    ids.iter()
        .map(String::as_str)
        .filter(|id| seen.insert(*id))
        .collect()
}
