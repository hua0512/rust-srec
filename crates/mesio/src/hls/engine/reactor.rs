//! The scheduler reactor: the single task that owns the control plane.
//!
//! Owns the `SegmentStateStore` (never shared, never locked), runs the
//! planner on each snapshot, drives bounded fetch-and-process tasks through a
//! `JoinSet`, applies their outcomes, and forwards `AssemblerInput` items
//! downstream via permit-reserve. The loop never blocks: network I/O lives in
//! the spawned tasks, AES in the crypto executor, and every `select!` arm
//! stays live under backpressure.
//!
//! Concurrency is bounded by the dispatch gate's
//! `inflight.len() < max_concurrency` check — the reactor is the sole
//! spawner, so the JoinSet length is the limit; no semaphore needed.

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::hls::HlsDownloaderError;

use super::fetch::{FetchContext, fetch_and_process};
use super::input::{AssemblerInput, PendingQueue, PlaylistNotice};
use super::planner::{PlannerContext, plan};
use super::store::{SegmentOutcome, SegmentStateStore, StoreConfig};
use super::watcher::{PlaylistSnapshot, TerminalCause};

/// Why the reactor stopped. Reported to the runtime for logging; the
/// consumer-visible effect (StreamEnded / nothing / `Err`) is delivered
/// through the assembler channel before this returns.
#[derive(Debug)]
pub enum Terminal {
    /// ENDLIST drained: every known segment completed, terminalized, or was
    /// skipped, and `AssemblerInput::End` was forwarded.
    AuthoritativeEnd,
    /// Caller-initiated stop. In-flight work aborted, nothing emitted.
    Cancelled,
    /// The assembler dropped its receiver. Surfaced, never swallowed.
    DownstreamClosed,
    /// Watcher failure, task panic, or other pipeline error. The error itself
    /// was forwarded as `AssemblerInput::Fatal`.
    PipelineError(Arc<str>),
}

pub struct ReactorConfig {
    pub store: StoreConfig,
    pub max_concurrency: usize,
    pub max_pending_payload_bytes: u64,
    pub max_pending_items: usize,
}

pub async fn run_reactor(
    mut playlist_rx: watch::Receiver<PlaylistSnapshot>,
    assembler_tx: mpsc::Sender<AssemblerInput>,
    mut planner_ctx: PlannerContext,
    fetch_ctx: Arc<FetchContext>,
    config: ReactorConfig,
    cancel: CancellationToken,
) -> Terminal {
    let mut store = SegmentStateStore::new(config.store.clone());
    let mut pending = PendingQueue::new();
    let mut inflight: JoinSet<SegmentOutcome> = JoinSet::new();
    let budget = Arc::clone(&fetch_ctx.budget);
    let max_concurrency = config.max_concurrency.max(1);

    // `ending`: a terminal cause was seen — drain known work, then finish.
    // `end_queued`: AssemblerInput::End enqueued exactly once.
    let mut ending = false;
    let mut end_queued = false;

    // Pipeline-error exits forward the error as Fatal so the consumer sees a
    // terminal Err, not a silent close.
    let mut fatal: Option<HlsDownloaderError> = None;

    // The generation-0 snapshot (which for VOD already carries ENDLIST) is
    // retained in the channel; process it before entering the loop.
    {
        let snapshot = playlist_rx.borrow_and_update().clone();
        if let Some(err) = process_snapshot(
            &snapshot,
            &mut store,
            &mut planner_ctx,
            &mut pending,
            &mut ending,
        ) {
            let reason: Arc<str> = Arc::from(err.to_string());
            let _ = assembler_tx.send(AssemblerInput::Fatal(err)).await;
            return Terminal::PipelineError(reason);
        }
    }

    let terminal = loop {
        // Authoritative end: enqueue End only once *all lifecycle work is
        // finished* — `has_unfinished_work()` includes future-deadline
        // retries, so a final-window segment waiting on a retry holds the
        // stream open (the retry-deadline arm wakes the loop to run it).
        if ending && !store.has_unfinished_work() {
            if !end_queued {
                pending.push(AssemblerInput::End);
                end_queued = true;
            } else if pending.is_empty() {
                // End has been forwarded; nothing left to do.
                break Terminal::AuthoritativeEnd;
            }
        }

        // Top-up gate: slots ∧ pending-bytes ∧ pending-items. Download bytes
        // gate via the admission reservation inside `next_ready_jobs`, not a
        // separate read. This runs while `ending` too — the final ENDLIST
        // window is ready work that must download before the drain above can
        // be satisfied.
        if inflight.len() < max_concurrency
            && pending.payload_bytes() < config.max_pending_payload_bytes
            && pending.len() < config.max_pending_items
        {
            let slots = max_concurrency - inflight.len();
            let (jobs, admission_inputs) = store.next_ready_jobs(slots, Instant::now(), &budget);
            for input in admission_inputs {
                pending.push(input);
            }
            for job in jobs {
                inflight.spawn(fetch_and_process(job, Arc::clone(&fetch_ctx)));
            }
        }

        // Promote due retries on every pass — not only at admission. If a
        // deadline elapsed while the dispatch gate was closed, the entry must
        // leave the retry heap (it becomes Queued ready work) or
        // `next_retry_deadline` would keep returning a past instant and the
        // retry-wake arm below would complete instantly on every poll,
        // busy-spinning the loop until the gate reopens.
        store.promote_due_retries(Instant::now());

        // `next_retry_deadline` returns None when no retry is pending; the
        // arm is disabled rather than spinning.
        let retry_deadline = store.next_retry_deadline();

        tokio::select! {
            _ = cancel.cancelled() => break Terminal::Cancelled,

            // Discovery. `if !ending` disables this arm once ending starts,
            // so a closed watch (changed() erring forever) cannot hot-loop.
            // Also gated on pending capacity so snapshot-derived pushes
            // cannot grow `pending` unbounded — the watch retains the latest
            // value, nothing is lost while suspended.
            changed = playlist_rx.changed(),
                if !ending && pending.len() < config.max_pending_items =>
            {
                match changed {
                    Ok(()) => {
                        let snapshot = playlist_rx.borrow_and_update().clone();
                        if let Some(err) = process_snapshot(
                            &snapshot,
                            &mut store,
                            &mut planner_ctx,
                            &mut pending,
                            &mut ending,
                        ) {
                            fatal = Some(err);
                            break Terminal::PipelineError(Arc::from("playlist failure"));
                        }
                    }
                    // Sender dropped WITHOUT a terminal snapshot first: the
                    // watcher died, not a clean end. (An unseen terminal value
                    // is always delivered as Ok before changed() errors.)
                    Err(_) => {
                        if cancel.is_cancelled() {
                            break Terminal::Cancelled;
                        }
                        fatal = Some(HlsDownloaderError::Playlist {
                            reason: "playlist watcher stopped without signalling a cause"
                                .to_string(),
                        });
                        break Terminal::PipelineError(Arc::from("watcher failed"));
                    }
                }
            }

            // Completion: apply_outcome returns the assembler items this
            // outcome implies (Payload on success, TerminalFailed when the
            // store terminalizes). All cross the same ordered boundary.
            Some(joined) = inflight.join_next() => {
                match joined {
                    Ok(outcome) => {
                        for item in store.apply_outcome(outcome, Instant::now()) {
                            pending.push(item);
                        }
                    }
                    Err(e) if e.is_cancelled() => {}
                    Err(e) => {
                        fatal = Some(HlsDownloaderError::Internal {
                            reason: format!("segment task panicked: {e}"),
                        });
                        break Terminal::PipelineError(Arc::from("segment task panicked"));
                    }
                }
            }

            // Forward: only when an item is buffered AND downstream has a
            // permit. Never `send().await` in an arm — that would stall every
            // other arm whenever the assembler is backpressured.
            permit = assembler_tx.reserve(), if !pending.is_empty() => {
                match permit {
                    Ok(permit) => permit.send(pending.pop().expect("guarded non-empty")),
                    Err(_) => break Terminal::DownstreamClosed,
                }
            }

            _ = tokio::time::sleep_until(
                tokio::time::Instant::from_std(
                    retry_deadline.unwrap_or_else(|| Instant::now() + std::time::Duration::from_secs(3600))
                )
            ), if retry_deadline.is_some() => {
                // Wake to promote due retries in the next top-up pass.
            }
        }
    };

    match &terminal {
        Terminal::AuthoritativeEnd => {
            info!("reactor finished: authoritative end (ENDLIST drained)");
        }
        Terminal::Cancelled => {
            debug!("reactor cancelled; aborting in-flight segment tasks");
            // JoinSet drop aborts the spawned tasks (the reason it is a
            // JoinSet and not detached handles).
        }
        Terminal::DownstreamClosed => {
            warn!("assembler channel closed; reactor stopping with error");
        }
        Terminal::PipelineError(reason) => {
            warn!(%reason, "reactor stopping on pipeline error");
        }
    }

    if let Some(err) = fatal {
        let reason: Arc<str> = Arc::from(err.to_string());
        // Buffered output is dropped, not drained, on the error path.
        pending.clear();
        let _ = assembler_tx.send(AssemblerInput::Fatal(err)).await;
        return Terminal::PipelineError(reason);
    }

    terminal
}

/// Plan and ingest one snapshot. Returns the pipeline error when the snapshot
/// carries `TerminalCause::Failed`.
fn process_snapshot(
    snapshot: &PlaylistSnapshot,
    store: &mut SegmentStateStore,
    planner_ctx: &mut PlannerContext,
    pending: &mut PendingQueue,
    ending: &mut bool,
) -> Option<HlsDownloaderError> {
    match &snapshot.terminal {
        Some(TerminalCause::Failed(reason)) => Some(HlsDownloaderError::Playlist {
            reason: format!("playlist refresh failed: {reason}"),
        }),
        terminal => {
            // An ENDLIST snapshot still carries the final window: plan and
            // ingest it *before* setting `ending`, or its segments are lost.
            let planned = plan(snapshot, planner_ctx);
            if planned.reset {
                // A media-sequence reset: output continuity cannot be
                // preserved (the assembler's emit cursor never regresses), so
                // surface it as a terminal pipeline error rather than silently
                // downloading-and-discarding the re-based window forever.
                return Some(HlsDownloaderError::Playlist {
                    reason: "media-sequence reset on the playlist; stream must restart".to_string(),
                });
            }
            let stats = store.ingest(planned.descriptors, Instant::now());
            debug!(
                generation = snapshot.generation,
                discovered = stats.discovered,
                refreshed = stats.refreshed,
                deduplicated = stats.deduplicated,
                missing = planned.missing.len(),
                skipped = planned.skipped.len(),
                "snapshot planned"
            );
            for (from_msn, to_msn) in planned.missing {
                pending.push(AssemblerInput::Skipped { from_msn, to_msn });
            }
            for (from_msn, to_msn) in planned.skipped {
                pending.push(AssemblerInput::Skipped { from_msn, to_msn });
            }
            // Snapshot-derived notices cross the same ordered boundary as
            // payloads, keeping the client channel single-producer.
            pending.push(AssemblerInput::Notice(PlaylistNotice::PlaylistRefreshed {
                media_sequence_base: snapshot.playlist.media_sequence,
                target_duration: snapshot.playlist.target_duration as f64,
            }));
            if matches!(terminal, Some(TerminalCause::Endlist)) {
                pending.push(AssemblerInput::Notice(PlaylistNotice::EndlistEncountered));
                *ending = true;
            }
            // Prune under the window invariant: only entries below the new
            // window start are eligible (see SegmentStateStore::prune_below).
            store.prune_below(snapshot.playlist.media_sequence);
            None
        }
    }
}
