//! The single typed boundary between the reactor and the assembler.
//!
//! Payloads, skips, terminal failures, playlist notices, fatal errors, and
//! end-of-stream all cross the same ordered channel. Without the control
//! items, a segment the store has marked terminal would leave the assembler
//! blocked on an MSN that can never complete.

use std::collections::VecDeque;

use crate::hls::HlsDownloaderError;

use super::identity::SegmentKey;
use super::payload::SegmentPayload;

/// Playlist-level notice derived by the reactor from a planned snapshot. The
/// assembler forwards these to the sink immediately, never reordered, so the
/// consumer-facing channel keeps exactly one producer.
#[derive(Debug, Clone)]
pub enum PlaylistNotice {
    PlaylistRefreshed {
        media_sequence_base: u64,
        target_duration: f64,
    },
    EndlistEncountered,
}

#[derive(Debug)]
pub enum AssemblerInput {
    Payload(SegmentPayload),
    /// The store/planner gave up on these MSNs (window slide, ad filtering, a
    /// gap-skip decision, or terminal failure of a segment the assembler is
    /// waiting on). The assembler must stop waiting and advance past them.
    Skipped {
        from_msn: u64,
        to_msn: u64,
    },
    /// A specific segment will never arrive (terminal failure).
    TerminalFailed {
        key: SegmentKey,
        msn: u64,
    },
    Notice(PlaylistNotice),
    /// Pipeline error: drop the reorder buffer and surface this as the
    /// stream's terminal `Err`. Never followed by `End`.
    Fatal(HlsDownloaderError),
    /// Authoritative end: drain the reorder buffer in order, then emit
    /// `StreamEnded`. Arrives only on the ENDLIST path, so a channel close
    /// *without* a preceding `End` is a cancel/error and the buffer is
    /// dropped, not drained.
    End,
}

/// Reactor-local buffer between completion and the downstream permit-send.
///
/// Bounded in two dimensions by its producers (the dispatch gate and the
/// snapshot-intake guard), never by dropping events: payload bytes against
/// `max_pending_payload_bytes`, total items against `max_pending_items`.
/// `payload_bytes` is a running counter maintained on push/pop — never
/// recomputed by walking the buffer.
#[derive(Debug, Default)]
pub struct PendingQueue {
    items: VecDeque<AssemblerInput>,
    payload_bytes: u64,
}

impl PendingQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn payload_bytes(&self) -> u64 {
        self.payload_bytes
    }

    /// Push an item, coalescing adjacent/overlapping `Skipped` ranges so a
    /// wide window-slide is one control item, not thousands. This bounds
    /// control items independently of how wide a slide is and complements the
    /// `max_pending_items` cap.
    pub fn push(&mut self, item: AssemblerInput) {
        if let AssemblerInput::Skipped { from_msn, to_msn } = &item
            && let Some(AssemblerInput::Skipped {
                from_msn: tail_from,
                to_msn: tail_to,
            }) = self.items.back_mut()
        {
            // Adjacent or overlapping with the tail item: extend it in place.
            let extendable = *from_msn <= tail_to.saturating_add(1) && *to_msn >= *tail_from;
            if extendable {
                *tail_from = (*tail_from).min(*from_msn);
                *tail_to = (*tail_to).max(*to_msn);
                return;
            }
        }
        if let AssemblerInput::Payload(p) = &item {
            self.payload_bytes += p.len() as u64;
        }
        self.items.push_back(item);
    }

    pub fn pop(&mut self) -> Option<AssemblerInput> {
        let item = self.items.pop_front()?;
        if let AssemblerInput::Payload(p) = &item {
            self.payload_bytes = self.payload_bytes.saturating_sub(p.len() as u64);
        }
        Some(item)
    }

    /// Drop everything buffered. Used on the pipeline-error path, where the
    /// spec is to drop (not drain) buffered output before surfacing the error.
    pub fn clear(&mut self) {
        self.items.clear();
        self.payload_bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skipped(from: u64, to: u64) -> AssemblerInput {
        AssemblerInput::Skipped {
            from_msn: from,
            to_msn: to,
        }
    }

    #[test]
    fn adjacent_skipped_ranges_coalesce() {
        let mut q = PendingQueue::new();
        q.push(skipped(10, 12));
        q.push(skipped(13, 20));
        assert_eq!(q.len(), 1);
        match q.pop().unwrap() {
            AssemblerInput::Skipped { from_msn, to_msn } => {
                assert_eq!((from_msn, to_msn), (10, 20));
            }
            other => panic!("expected skipped, got {other:?}"),
        }
    }

    #[test]
    fn overlapping_skipped_ranges_coalesce() {
        let mut q = PendingQueue::new();
        q.push(skipped(10, 15));
        q.push(skipped(12, 13));
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn non_adjacent_skipped_ranges_do_not_coalesce() {
        let mut q = PendingQueue::new();
        q.push(skipped(10, 12));
        q.push(skipped(20, 25));
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn skipped_does_not_coalesce_across_other_items() {
        let mut q = PendingQueue::new();
        q.push(skipped(1, 2));
        q.push(AssemblerInput::Notice(PlaylistNotice::EndlistEncountered));
        q.push(skipped(3, 4));
        assert_eq!(q.len(), 3);
    }
}
