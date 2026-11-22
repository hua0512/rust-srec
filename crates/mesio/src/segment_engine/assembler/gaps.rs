//! Dead-range tracking and gap-skip evaluation for [`SequenceAssembler`].

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use tracing::debug;

use super::super::config::GapSkipStrategy;
use super::{AssembledItem, AssemblerSink, GapSkipReason, SequenceAssembler};

/// Coalesced set of MSNs the upstream declared dead (window slides, ads,
/// terminal failures). When the emit cursor reaches a dead MSN, the assembler
/// advances past the whole run instead of waiting on it.
#[derive(Debug, Default)]
pub(super) struct DeadRanges {
    /// from -> to (inclusive), non-overlapping, non-adjacent.
    ranges: BTreeMap<u64, u64>,
}

impl DeadRanges {
    pub(super) fn insert(&mut self, from: u64, to: u64) {
        let (mut from, mut to) = (from.min(to), from.max(to));
        // Merge any range overlapping or adjacent to [from, to].
        loop {
            let merge = self
                .ranges
                .range(..=to.saturating_add(1))
                .next_back()
                .filter(|&(_, &t)| t.saturating_add(1) >= from)
                .map(|(&f, &t)| (f, t));
            match merge {
                Some((f, t)) => {
                    self.ranges.remove(&f);
                    from = from.min(f);
                    to = to.max(t);
                }
                None => break,
            }
        }
        self.ranges.insert(from, to);
    }

    /// If `msn` is dead, the inclusive end of its run.
    pub(super) fn run_end(&self, msn: u64) -> Option<u64> {
        self.ranges
            .range(..=msn)
            .next_back()
            .filter(|&(_, &to)| to >= msn)
            .map(|(_, &to)| to)
    }

    pub(super) fn prune_below(&mut self, msn: u64) {
        self.ranges.retain(|_, to| *to >= msn);
    }
}

#[derive(Debug)]
pub(super) struct GapState {
    pub(super) missing_sequence: u64,
    pub(super) detected_at: Instant,
    pub(super) paused_at_detection: Duration,
    pub(super) segments_since_gap: u64,
}

impl GapState {
    pub(super) fn new(missing_sequence: u64, paused_at_detection: Duration) -> Self {
        Self {
            missing_sequence,
            detected_at: Instant::now(),
            paused_at_detection,
            segments_since_gap: 0,
        }
    }
}

impl<S: AssemblerSink> SequenceAssembler<S> {
    pub(super) fn gap_strategy(&self) -> &GapSkipStrategy {
        if self.is_live_stream {
            &self.config.output.live_gap_strategy
        } else {
            &self.config.output.vod_gap_strategy
        }
    }

    /// At-cap detection for the forced-skip rule: when the reorder buffer is
    /// over its byte/segment limit and blocked on a gap, the gap policy's
    /// count/duration thresholds are bypassed.
    pub(super) fn buffer_at_limit(&self) -> bool {
        let limits = &self.config.output.buffer_limits;
        (limits.max_segments > 0 && self.reorder_buffer.len() >= limits.max_segments)
            || (limits.max_bytes > 0 && self.current_buffer_bytes >= limits.max_bytes)
    }

    /// Time a gap has waited, excluding the credited rate-limit pause
    /// since it was detected.
    pub(super) fn gap_elapsed(&self, gap_state: &GapState, now: Instant) -> Duration {
        let paused = self
            .rate_limit_holds
            .clock()
            .credited_pause(gap_state.paused_at_detection, now);
        now.saturating_duration_since(gap_state.detected_at)
            .saturating_sub(paused)
    }

    pub(super) fn should_skip_gap(&self) -> Option<GapSkipReason> {
        let gap_state = self.gap_state.as_ref()?;

        // Buffer pressure overrides thresholds: waiting at the cap recreates
        // the deadlock the keep-draining rule exists to prevent.
        //
        // A stream that waits for every segment is exempt: the reactor keeps
        // what is buffered near the limits, and the assembler keeps draining
        // its input either way, so waiting on the missing segment loses
        // nothing. Giving it up would silently drop a segment that is still
        // coming.
        let waits_for_every_segment = !self.is_live_stream
            && matches!(self.gap_strategy(), GapSkipStrategy::WaitIndefinitely);
        if self.buffer_at_limit() && !waits_for_every_segment {
            return Some(GapSkipReason::BufferPressure);
        }

        let in_startup =
            self.is_live_stream && self.is_fmp4_stream && !self.has_emitted_media_segment;
        let elapsed = self.gap_elapsed(gap_state, Instant::now());

        match self.gap_strategy() {
            GapSkipStrategy::WaitIndefinitely => None,
            GapSkipStrategy::SkipAfterCount(threshold) => {
                if !in_startup && gap_state.segments_since_gap >= *threshold {
                    Some(GapSkipReason::CountThreshold(gap_state.segments_since_gap))
                } else {
                    None
                }
            }
            GapSkipStrategy::SkipAfterDuration(threshold) => {
                (elapsed >= *threshold).then_some(GapSkipReason::DurationThreshold(elapsed))
            }
            GapSkipStrategy::SkipAfterBoth { count, duration } => {
                let duration_exceeded = elapsed >= *duration;
                let count_exceeded = !in_startup && gap_state.segments_since_gap >= *count;
                if count_exceeded && duration_exceeded {
                    Some(GapSkipReason::BothThresholds {
                        count: gap_state.segments_since_gap,
                        duration: elapsed,
                    })
                } else if count_exceeded {
                    Some(GapSkipReason::CountThreshold(gap_state.segments_since_gap))
                } else if duration_exceeded {
                    Some(GapSkipReason::DurationThreshold(elapsed))
                } else {
                    None
                }
            }
        }
    }

    pub(super) fn record_gap_skip(&self, to: u64) {
        self.metrics.gap_skips.fetch_add(1, Ordering::Relaxed);
        self.metrics.total_segments_skipped.fetch_add(
            to.saturating_sub(self.expected_next_media_sequence),
            Ordering::Relaxed,
        );
    }

    pub(super) async fn emit_gap_skipped(
        &mut self,
        from: u64,
        to: u64,
        reason: GapSkipReason,
    ) -> Result<(), ()> {
        self.metrics.gap_skips.fetch_add(1, Ordering::Relaxed);
        self.metrics
            .total_segments_skipped
            .fetch_add(to.saturating_sub(from), Ordering::Relaxed);
        self.sink
            .send(Ok(AssembledItem::GapSkipped {
                from_sequence: from,
                to_sequence: to,
                reason,
            }))
            .await
            .map_err(|_| ())
    }

    pub(super) async fn skip_buffered_media_as_gap(
        &mut self,
        msn: u64,
        reason: GapSkipReason,
        advance_cursor: bool,
    ) -> Result<(), ()> {
        let Some(to) = msn.checked_add(1) else {
            self.abort_u64_max_cursor().await;
            return Err(());
        };
        if self.remove_buffered_payload(msn).is_none() {
            debug!(msn, "gap skip requested for missing buffered media");
            return Ok(());
        }
        self.emit_gap_skipped(msn, to, reason).await?;
        if advance_cursor {
            self.advance_to(to);
        }
        Ok(())
    }
}
