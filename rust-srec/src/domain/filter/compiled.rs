//! Bounded reuse across domain objects reconstructed from persisted filters on every poll.

use std::collections::VecDeque;
use std::str::FromStr;
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;

use super::FilterEvalError;

const CACHE_ENTRIES: usize = 64;
const MAX_CACHED_SOURCE_BYTES: usize = 16 * 1024;

type Compiled<V> = Arc<OnceLock<Result<Arc<V>, FilterEvalError>>>;

struct DefinitionCache<K, V> {
    capacity: usize,
    entries: Mutex<VecDeque<(K, Compiled<V>)>>,
}

impl<K: Eq, V> DefinitionCache<K, V> {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: Mutex::new(VecDeque::new()),
        }
    }

    fn get_or_compile(
        &self,
        key: K,
        compile: impl FnOnce() -> Result<V, FilterEvalError>,
    ) -> Result<Arc<V>, FilterEvalError> {
        let cell = {
            let mut entries = self.entries.lock();
            let hit = entries
                .iter()
                .position(|(existing, _)| existing == &key)
                .and_then(|position| entries.remove(position));
            if let Some((key, cell)) = hit {
                entries.push_back((key, cell.clone()));
                cell
            } else {
                let cell = Arc::new(OnceLock::new());
                if entries.len() >= self.capacity {
                    drop(entries.pop_front());
                }
                entries.push_back((key, cell.clone()));
                cell
            }
        };
        // Compilation never holds the shared LRU lock. Concurrent users of one admitted
        // definition share a cell, including invalid definitions; edits have a different key.
        cell.get_or_init(|| compile().map(Arc::new)).clone()
    }
}

pub(super) fn cron(expression: &str) -> Result<Arc<cron::Schedule>, FilterEvalError> {
    let compile = || {
        cron::Schedule::from_str(expression)
            .map_err(|error| FilterEvalError::InvalidCronExpression(error.to_string()))
    };
    if expression.len() > MAX_CACHED_SOURCE_BYTES {
        return compile().map(Arc::new);
    }
    static CACHE: OnceLock<DefinitionCache<String, cron::Schedule>> = OnceLock::new();
    CACHE
        .get_or_init(|| DefinitionCache::new(CACHE_ENTRIES))
        .get_or_compile(expression.to_owned(), compile)
}

pub(super) fn regex(
    pattern: &str,
    case_insensitive: bool,
) -> Result<Arc<regex::Regex>, FilterEvalError> {
    let compile = || {
        regex::RegexBuilder::new(pattern)
            .case_insensitive(case_insensitive)
            .build()
            .map_err(|error| FilterEvalError::InvalidRegexPattern(error.to_string()))
    };
    if pattern.len() > MAX_CACHED_SOURCE_BYTES {
        return compile().map(Arc::new);
    }
    static CACHE: OnceLock<DefinitionCache<(String, bool), regex::Regex>> = OnceLock::new();
    CACHE
        .get_or_init(|| DefinitionCache::new(CACHE_ENTRIES))
        .get_or_compile((pattern.to_owned(), case_insensitive), compile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_rules_are_reused_across_reconstruction_and_edits_use_new_keys() {
        assert!(Arc::ptr_eq(
            &cron("0 0 22 * * *").unwrap(),
            &cron("0 0 22 * * *").unwrap()
        ));
        assert!(Arc::ptr_eq(
            &regex("abc", false).unwrap(),
            &regex("abc", false).unwrap()
        ));
        assert!(!regex("abc", false).unwrap().is_match("ABC"));
        assert!(regex("abc", true).unwrap().is_match("ABC"));
        assert!(!regex("xyz", true).unwrap().is_match("ABC"));
    }
}
