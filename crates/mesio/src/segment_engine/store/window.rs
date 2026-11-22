//! Admission window bounds and live sliding-window pruning for [`SegmentStateStore`].

use std::collections::HashSet;
use std::time::Instant;

use tracing::{debug, warn};

use super::super::identity::{SegmentKey, SegmentKind};
use super::super::input::AssemblerInput;
use super::{SegmentState, SegmentStateStore};

/// How far past the emit cursor media is admitted, for a stream that waits
/// for every segment. The assembler skips the segment it waits for once its
/// reorder buffer holds `max_segments` segments or `max_bytes` bytes, so
/// admission keeps what can pile up behind that segment below both.
#[derive(Debug, Clone, Copy)]
pub struct MediaWindow {
    /// The next MSN the assembler emits or skips.
    pub cursor: u64,
    /// Media at or beyond `cursor + segments` waits (0 = no limit).
    pub segments: u64,
    /// Media waits while the media at or beyond the cursor, downloading or
    /// complete, would exceed this many bytes (0 = no limit). The estimate
    /// of a segment still downloading may be off, so this stays below the
    /// assembler's own byte limit.
    pub bytes: u64,
}

impl SegmentStateStore {
    /// Prune for long-running live streams under one invariant: **never evict
    /// an entry whose key can still appear in the playlist window** — evicting
    /// a `Completed` record still in the window would make the next refresh
    /// re-download it. Only finished records below `window_start_msn` are
    /// eligible, and init records are retained up to `max_retained_inits`
    /// (newest first). `max_state_entries` is a backstop within that rule:
    /// when nothing is safely evictable, state temporarily exceeds the cap.
    ///
    /// Unfinished media below the window (discovered, queued, or awaiting a
    /// retry) is the backlog: a segment that slid out of the playlist is
    /// usually still served for a while, so it is kept and fetched rather
    /// than lost. The backlog is bounded to one window's worth
    /// (`window_len`) and by `max_state_entries`; beyond that, downloads are
    /// persistently slower than the stream and keeping more only grows memory
    /// and lag. The oldest entries go first, and each dropped MSN is returned
    /// as a `Skipped` input — the assembler waits on every planned MSN, so it
    /// must be told. In-flight work and unfinished inits are never dropped.
    pub fn prune_below(&mut self, window_start_msn: u64, window_len: usize) -> Vec<AssemblerInput> {
        self.records.retain(|_, record| {
            record.state.is_unfinished()
                || record.descriptor.key.kind == SegmentKind::Init
                || record.descriptor.msn >= window_start_msn
        });
        let dropped = self.drop_excess_backlog(window_start_msn, window_len);

        // Retain only the newest N init records, but never evict one that can
        // still appear in the window (msn >= window_start_msn) — that would
        // make the next refresh re-discover and re-download it, breaking the
        // same prune invariant the media retain above upholds. Unfinished
        // inits are never evicted either: dependent media is gated on them.
        // Nor is a failed init that unfinished media needs: it keeps that
        // media from being fetched (see `failed_init_of`).
        let needed: HashSet<&SegmentKey> = self
            .records
            .values()
            .filter(|r| r.state.is_unfinished())
            .filter_map(|r| r.descriptor.init_key.as_ref())
            .collect();
        let total_finished_inits = self
            .records
            .values()
            .filter(|r| r.descriptor.key.kind == SegmentKind::Init && !r.state.is_unfinished())
            .count();
        let excess = total_finished_inits.saturating_sub(self.config.max_retained_inits);
        if excess > 0 {
            let mut init_msns: Vec<(u64, SegmentKey)> = self
                .records
                .iter()
                .filter(|(key, r)| {
                    r.descriptor.key.kind == SegmentKind::Init
                        && !r.state.is_unfinished()
                        && r.latest_msn < window_start_msn
                        && !(matches!(r.state, SegmentState::TerminalFailed { .. })
                            && needed.contains(key))
                })
                .map(|(k, r)| (r.latest_msn, k.clone()))
                .collect();
            init_msns.sort_by_key(|(msn, _)| *msn);
            for (_, key) in init_msns.into_iter().take(excess) {
                self.records.remove(&key);
            }
        }

        if self.records.len() > self.config.max_state_entries {
            // Everything still here is in-window, in-flight, or a retained
            // init — not safely evictable. Honoring the invariant beats the
            // cap (see StoreConfig::max_state_entries).
            debug!(
                records = self.records.len(),
                cap = self.config.max_state_entries,
                "state store over capacity; all remaining entries are protected"
            );
        }
        // Ready/heap entries for pruned keys are dropped lazily on pop.
        dropped
    }

    fn drop_excess_backlog(
        &mut self,
        window_start_msn: u64,
        window_len: usize,
    ) -> Vec<AssemblerInput> {
        let mut backlog: Vec<(u64, SegmentKey)> = self
            .records
            .iter()
            .filter(|(_, r)| {
                r.descriptor.key.kind == SegmentKind::Media
                    && r.descriptor.msn < window_start_msn
                    && r.state.is_unfinished()
                    && !matches!(r.state, SegmentState::InFlight)
            })
            .map(|(key, r)| (r.descriptor.msn, key.clone()))
            .collect();
        let over_state_cap = self
            .records
            .len()
            .saturating_sub(self.config.max_state_entries);
        let excess = backlog
            .len()
            .saturating_sub(window_len)
            .max(over_state_cap)
            .min(backlog.len());
        if excess == 0 {
            return Vec::new();
        }

        backlog.sort_unstable_by_key(|(msn, _)| *msn);
        let mut dropped_msns = Vec::with_capacity(excess);
        for (msn, key) in backlog.into_iter().take(excess) {
            self.records.remove(&key);
            self.rate_limit_holds.release(&key, msn, Instant::now());
            dropped_msns.push(msn);
        }
        // Another record can share an MSN (a URI change re-keys a segment);
        // that MSN is still pending, so it must not be declared dead.
        let live_msns: HashSet<u64> = self
            .records
            .values()
            .filter(|r| r.descriptor.key.kind == SegmentKind::Media)
            .map(|r| r.descriptor.msn)
            .collect();
        dropped_msns.retain(|msn| !live_msns.contains(msn));
        dropped_msns.dedup();
        warn!(
            dropped = excess,
            window_start_msn, "download backlog exceeds its bound; skipping the oldest segments"
        );

        let mut inputs = Vec::new();
        let mut run: Option<(u64, u64)> = None;
        for msn in dropped_msns {
            run = match run {
                Some((from, to)) if msn == to.saturating_add(1) => Some((from, msn)),
                Some((from, to)) => {
                    inputs.push(AssemblerInput::Skipped {
                        from_msn: from,
                        to_msn: to,
                    });
                    Some((msn, msn))
                }
                None => Some((msn, msn)),
            };
        }
        if let Some((from_msn, to_msn)) = run {
            inputs.push(AssemblerInput::Skipped { from_msn, to_msn });
        }
        inputs
    }
}
