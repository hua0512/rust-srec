use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{debug, trace};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentJobKind {
    Init,
    Media,
    Prefetch,
}

#[derive(Debug, Clone)]
pub enum SegmentJobResult {
    Completed,
    Failed { retryable: bool, reason: String },
}

#[derive(Debug, Clone)]
pub struct SegmentJobOutcome {
    pub identity: Arc<str>,
    pub media_sequence_number: u64,
    pub kind: SegmentJobKind,
    pub result: SegmentJobResult,
}

#[derive(Debug, Clone, Copy)]
pub struct SegmentLifecycleConfig {
    pub max_entries: usize,
    pub retry_delay: Duration,
    pub max_reschedules: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentLifecycleState {
    InFlight,
    Completed,
    RetryableFailed,
    TerminalFailed,
}

#[derive(Debug, Clone)]
struct SegmentLifecycleEntry {
    media_sequence_number: u64,
    kind: SegmentJobKind,
    state: SegmentLifecycleState,
    reschedules: u32,
    next_retry_at: Option<Instant>,
}

pub struct SegmentLifecycleRegistry {
    config: SegmentLifecycleConfig,
    entries: HashMap<Arc<str>, SegmentLifecycleEntry>,
}

impl SegmentLifecycleRegistry {
    pub fn new(config: SegmentLifecycleConfig) -> Self {
        Self {
            config,
            entries: HashMap::new(),
        }
    }

    pub fn should_schedule(&self, identity: &str, now: Instant) -> bool {
        let Some(entry) = self.entries.get(identity) else {
            return true;
        };

        match entry.state {
            SegmentLifecycleState::InFlight
            | SegmentLifecycleState::Completed
            | SegmentLifecycleState::TerminalFailed => false,
            SegmentLifecycleState::RetryableFailed => entry
                .next_retry_at
                .is_none_or(|next_retry_at| now >= next_retry_at),
        }
    }

    pub fn mark_scheduled(
        &mut self,
        identity: Arc<str>,
        media_sequence_number: u64,
        kind: SegmentJobKind,
    ) {
        self.prune_to_capacity();

        self.entries
            .entry(identity)
            .and_modify(|entry| {
                if entry.state == SegmentLifecycleState::RetryableFailed {
                    entry.reschedules = entry.reschedules.saturating_add(1);
                }
                entry.media_sequence_number = media_sequence_number;
                entry.kind = kind;
                entry.state = SegmentLifecycleState::InFlight;
                entry.next_retry_at = None;
            })
            .or_insert_with(|| SegmentLifecycleEntry {
                media_sequence_number,
                kind,
                state: SegmentLifecycleState::InFlight,
                reschedules: 0,
                next_retry_at: None,
            });
    }

    pub fn apply_outcome(&mut self, outcome: SegmentJobOutcome, now: Instant) {
        let entry = self
            .entries
            .entry(Arc::clone(&outcome.identity))
            .or_insert_with(|| SegmentLifecycleEntry {
                media_sequence_number: outcome.media_sequence_number,
                kind: outcome.kind,
                state: SegmentLifecycleState::InFlight,
                reschedules: 0,
                next_retry_at: None,
            });

        entry.media_sequence_number = outcome.media_sequence_number;
        entry.kind = outcome.kind;

        match outcome.result {
            SegmentJobResult::Completed => {
                entry.state = SegmentLifecycleState::Completed;
                entry.next_retry_at = None;
                trace!(
                    msn = outcome.media_sequence_number,
                    identity = %outcome.identity,
                    "Segment lifecycle completed"
                );
            }
            SegmentJobResult::Failed { retryable, reason } => {
                if retryable && entry.reschedules < self.config.max_reschedules {
                    entry.state = SegmentLifecycleState::RetryableFailed;
                    entry.next_retry_at = Some(now + self.config.retry_delay);
                    debug!(
                        msn = outcome.media_sequence_number,
                        identity = %outcome.identity,
                        reschedules = entry.reschedules,
                        max_reschedules = self.config.max_reschedules,
                        reason = %reason,
                        "Segment lifecycle marked retryable"
                    );
                } else {
                    entry.state = SegmentLifecycleState::TerminalFailed;
                    entry.next_retry_at = None;
                    debug!(
                        msn = outcome.media_sequence_number,
                        identity = %outcome.identity,
                        retryable = retryable,
                        reschedules = entry.reschedules,
                        max_reschedules = self.config.max_reschedules,
                        reason = %reason,
                        "Segment lifecycle marked terminal"
                    );
                }
            }
        }
    }

    pub fn has_due_retry(&self, now: Instant) -> bool {
        self.entries.values().any(|entry| {
            entry.state == SegmentLifecycleState::RetryableFailed
                && entry.next_retry_at.is_none_or(|retry_at| now >= retry_at)
        })
    }

    pub fn time_until_next_retry(&self, now: Instant) -> Option<Duration> {
        self.entries
            .values()
            .filter(|entry| entry.state == SegmentLifecycleState::RetryableFailed)
            .filter_map(|entry| entry.next_retry_at)
            .map(|retry_at| retry_at.saturating_duration_since(now))
            .min()
    }

    pub fn prune_before_msn(&mut self, media_sequence_number: u64) {
        self.entries.retain(|_, entry| {
            entry.state == SegmentLifecycleState::InFlight
                || entry.kind == SegmentJobKind::Init
                || entry.media_sequence_number >= media_sequence_number
        });
        self.prune_to_capacity();
    }

    fn prune_to_capacity(&mut self) {
        if self.config.max_entries == 0 || self.entries.len() <= self.config.max_entries {
            return;
        }

        let remove_count = self.entries.len() - self.config.max_entries;
        let mut removable: Vec<(Arc<str>, u64)> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.state != SegmentLifecycleState::InFlight)
            .map(|(identity, entry)| (Arc::clone(identity), entry.media_sequence_number))
            .collect();
        removable.sort_by_key(|(_, msn)| *msn);

        for (identity, _) in removable.into_iter().take(remove_count) {
            self.entries.remove(identity.as_ref());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> SegmentLifecycleRegistry {
        SegmentLifecycleRegistry::new(SegmentLifecycleConfig {
            max_entries: 100,
            retry_delay: Duration::ZERO,
            max_reschedules: 2,
        })
    }

    #[test]
    fn in_flight_segment_is_not_schedulable_twice() {
        let mut registry = registry();
        let now = Instant::now();
        let identity = Arc::<str>::from("https://example.com/1.ts");

        assert!(registry.should_schedule(identity.as_ref(), now));
        registry.mark_scheduled(Arc::clone(&identity), 1, SegmentJobKind::Media);

        assert!(!registry.should_schedule(identity.as_ref(), now));
    }

    #[test]
    fn retryable_failure_becomes_schedulable_after_delay() {
        let mut registry = registry();
        let now = Instant::now();
        let identity = Arc::<str>::from("https://example.com/1.ts");

        registry.mark_scheduled(Arc::clone(&identity), 1, SegmentJobKind::Media);
        registry.apply_outcome(
            SegmentJobOutcome {
                identity: Arc::clone(&identity),
                media_sequence_number: 1,
                kind: SegmentJobKind::Media,
                result: SegmentJobResult::Failed {
                    retryable: true,
                    reason: "404".to_string(),
                },
            },
            now,
        );

        assert!(registry.should_schedule(identity.as_ref(), now));
    }

    #[test]
    fn completed_segment_is_not_schedulable_again() {
        let mut registry = registry();
        let now = Instant::now();
        let identity = Arc::<str>::from("https://example.com/1.ts");

        registry.mark_scheduled(Arc::clone(&identity), 1, SegmentJobKind::Media);
        registry.apply_outcome(
            SegmentJobOutcome {
                identity: Arc::clone(&identity),
                media_sequence_number: 1,
                kind: SegmentJobKind::Media,
                result: SegmentJobResult::Completed,
            },
            now,
        );

        assert!(!registry.should_schedule(identity.as_ref(), now));
    }

    #[test]
    fn retryable_failure_becomes_terminal_after_reschedule_budget() {
        let mut registry = registry();
        let now = Instant::now();
        let identity = Arc::<str>::from("https://example.com/1.ts");

        for _ in 0..=2 {
            registry.mark_scheduled(Arc::clone(&identity), 1, SegmentJobKind::Media);
            registry.apply_outcome(
                SegmentJobOutcome {
                    identity: Arc::clone(&identity),
                    media_sequence_number: 1,
                    kind: SegmentJobKind::Media,
                    result: SegmentJobResult::Failed {
                        retryable: true,
                        reason: "404".to_string(),
                    },
                },
                now,
            );
        }

        assert!(!registry.should_schedule(identity.as_ref(), now));
    }
}
