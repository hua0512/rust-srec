//! Retained and failed initialization-segment tracking for [`super::SequenceAssembler`].

use std::collections::{HashMap, VecDeque};

use crate::error::SharedError;

use super::super::identity::SegmentKey;
use super::super::payload::SegmentPayload;

/// Bounded insertion-ordered map of failed init keys to their failure. Init
/// rotations are infrequent (discontinuities / rendition changes), so a small
/// cap with FIFO eviction keeps memory bounded on long-running streams.
/// Eviction is safe because an unknown key is treated as unresolved: media
/// still waits in the normal case, and the live buffer-pressure / ENDLIST
/// flush fail-safes surface stuck media as visible gaps.
#[derive(Debug)]
pub(super) struct FailedInits {
    cap: usize,
    order: VecDeque<SegmentKey>,
    errors: HashMap<SegmentKey, SharedError>,
}

impl FailedInits {
    pub(super) fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            order: VecDeque::new(),
            errors: HashMap::new(),
        }
    }

    pub(super) fn insert(&mut self, key: SegmentKey, error: SharedError) {
        if self.errors.insert(key.clone(), error).is_none() {
            self.order.push_back(key);
            while self.order.len() > self.cap {
                if let Some(oldest) = self.order.pop_front() {
                    self.errors.remove(&oldest);
                }
            }
        }
    }

    pub(super) fn get(&self, key: &SegmentKey) -> Option<&SharedError> {
        self.errors.get(key)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Received init payloads by key, retained so an init can be emitted — again,
/// if the stream switches back to it — whenever media at the cursor needs it.
///
/// Bounded least-recently-used among the entries `is_protected` does not
/// cover (the active init, any init referenced by buffered media, any init at
/// or beyond the emit cursor, and any unemitted init whose governing MSN lies
/// ahead of the last emitted media segment). A finished stream fetches its
/// inits ahead of their media, and evicting one of those would leave that
/// media waiting for a map that is not fetched again. An evicted key is
/// unresolved again, with the same fail-safes as [`FailedInits`]; the store
/// sends its copy of a completed init again when the media it admits switches
/// to that init.
#[derive(Debug)]
pub(super) struct RetainedInits {
    /// `0` disables the limit.
    cap: usize,
    /// Logical clock for recency: touching an entry is O(1) on every media
    /// emission; only an over-cap insert scans for the least recent entry.
    clock: u64,
    entries: HashMap<SegmentKey, RetainedInit>,
}

#[derive(Debug)]
pub(super) struct RetainedInit {
    pub(super) payload: SegmentPayload,
    pub(super) emitted: bool,
    pub(super) last_used: u64,
}

impl RetainedInits {
    pub(super) fn new(cap: usize) -> Self {
        Self {
            cap,
            clock: 0,
            entries: HashMap::new(),
        }
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// Inserts or updates `payload`, evicting the least-recently-used
    /// unprotected entry when over capacity.
    pub(super) fn insert(
        &mut self,
        payload: SegmentPayload,
        mut is_protected: impl FnMut(&SegmentKey, &RetainedInit) -> bool,
    ) {
        let key = payload.descriptor().key.clone();
        let now = self.tick();
        match self.entries.get_mut(&key) {
            Some(entry) => {
                // The store sends a completed init again without its
                // discontinuity (see `SegmentPayload::replayed_at`). An
                // entry that has not been emitted still owes the one it
                // arrived with, so it keeps it.
                let owed = !entry.emitted && entry.payload.discontinuity();
                entry.payload = if owed {
                    payload.with_discontinuity()
                } else {
                    payload
                };
                entry.last_used = now;
            }
            None => {
                self.entries.insert(
                    key,
                    RetainedInit {
                        payload,
                        emitted: false,
                        last_used: now,
                    },
                );
            }
        }
        while self.cap > 0 && self.entries.len() > self.cap {
            let Some(oldest) = self
                .entries
                .iter()
                .filter(|(k, entry)| !is_protected(k, entry))
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    pub(super) fn contains(&self, key: &SegmentKey) -> bool {
        self.entries.contains_key(key)
    }

    /// A copy of the payload to emit, and whether this is its first emission.
    pub(super) fn take_for_emission(&mut self, key: &SegmentKey) -> Option<(SegmentPayload, bool)> {
        let now = self.tick();
        let entry = self.entries.get_mut(key)?;
        let first = !entry.emitted;
        entry.emitted = true;
        entry.last_used = now;
        Some((entry.payload.clone(), first))
    }

    /// Mark `key` as used; a no-op if it is not retained.
    pub(super) fn touch(&mut self, key: &SegmentKey) {
        let now = self.tick();
        if let Some(entry) = self.entries.get_mut(key) {
            entry.last_used = now;
        }
    }
}
