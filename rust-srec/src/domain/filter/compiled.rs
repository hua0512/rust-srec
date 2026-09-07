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
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    #[test]
    fn lru_is_bounded_and_failed_compilation_is_reused() {
        let cache = DefinitionCache::<u8, u8>::new(2);
        let calls = AtomicUsize::new(0);
        for _ in 0..2 {
            assert!(
                cache
                    .get_or_compile(1, || {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Err(FilterEvalError::InvalidRegexPattern("invalid".to_owned()))
                    })
                    .is_err()
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        cache.get_or_compile(2, || Ok(2)).unwrap();
        cache.get_or_compile(3, || Ok(3)).unwrap();
        assert_eq!(cache.entries.lock().len(), 2);
        assert_eq!(
            *cache.get_or_compile(1, || Ok(1)).unwrap(),
            1,
            "oldest entry was evicted"
        );
    }

    #[test]
    fn concurrent_compilation_shares_one_result_and_invalid_inputs_stay_errors() {
        let cache = DefinitionCache::<u8, u8>::new(2);
        let calls = AtomicUsize::new(0);
        let barrier = std::sync::Barrier::new(4);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        cache
                            .get_or_compile(1, || {
                                calls.fetch_add(1, Ordering::SeqCst);
                                Ok(1)
                            })
                            .unwrap()
                    })
                })
                .collect();
            let values: Vec<_> = handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect();
            assert!(values.iter().all(|value| Arc::ptr_eq(value, &values[0])));
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        for _ in 0..2 {
            assert!(matches!(
                cron("not cron"),
                Err(FilterEvalError::InvalidCronExpression(_))
            ));
            assert!(matches!(
                regex("(", false),
                Err(FilterEvalError::InvalidRegexPattern(_))
            ));
        }
    }

    #[test]
    fn oversized_sources_are_not_retained_by_the_bounded_cache() {
        let pattern = "a".repeat(MAX_CACHED_SOURCE_BYTES + 1);
        let first = regex(&pattern, false).unwrap();
        let second = regex(&pattern, false).unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
    }
}
