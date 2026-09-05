//! Taking a deleted streamer off the runtime before its row is removed.
//!
//! A streamer id is held by four owners that can each write rows for it: the
//! scheduler's `StreamerActor`, an in-flight download attempt, the session
//! lifecycle, and the pipeline jobs of its sessions. Deleting the row while any
//! of them still holds it is what the `deleted_at` marker exists to avoid:
//! marking makes `StreamerMetadata::is_active` false so no owner starts new
//! work, [`RuntimeCoordinator::retire_streamer`] waits for the work already in
//! flight, and only then may `StreamerManager::reap_deleted` run the delete.

use std::time::Duration;

use tracing::{debug, info, warn};

use crate::scheduler::actor::ActorRemovalOutcome;

use super::{RuntimeCoordinator, StopWait};

/// How long each owner is given to acknowledge behind one user request:
/// `DELETE /api/streamers/{id}`, the MCP tool that calls it, and a configuration
/// import.
///
/// Sized for the runtime standing down inside an HTTP request, not for
/// post-processing: the actor bound covers one `StreamerActor::check_status`
/// platform round trip, the download bound an engine's graceful stop, and the
/// pipeline bound only the rows a just-ended session still has open. A streamer
/// whose session-complete pipeline is transcoding or uploading will not drain
/// inside it, which is why the deletion is already durable in
/// `streamers.deleted_at` before any of this is awaited and
/// `ServiceContainer::spawn_streamer_reaper` finishes what these bounds cut off.
pub(crate) const INTERACTIVE_RETIREMENT: RetirementBounds = RetirementBounds {
    actor: Duration::from_secs(10),
    download: Duration::from_secs(20),
    pipeline: Duration::from_secs(2),
};

/// One observation of each owner, with no waiting.
///
/// Used by the reaper sweep, which has its own cadence so waiting inside a tick
/// buys nothing the next tick does not, and by `BatchStreamerAction::Delete`,
/// where up to `MAX_BATCH_SIZE` retirements run in sequence and per-streamer
/// bounds would multiply into one request. Both leave anything still in flight
/// marked for the next sweep.
pub(crate) const OBSERVE_RETIREMENT: RetirementBounds = RetirementBounds {
    actor: Duration::ZERO,
    download: Duration::ZERO,
    pipeline: Duration::ZERO,
};

/// Per-owner bounds for one [`RuntimeCoordinator::retire_streamer`] call.
///
/// Every wait belongs to the caller; nothing in the retirement path imposes a
/// timeout of its own.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RetirementBounds {
    /// Bound on the scheduler actor's task leaving the runtime.
    pub actor: Duration,
    /// Bound on each download attempt publishing its terminal outcome.
    pub download: Duration,
    /// Bound on the streamer's session reaching pipeline quiescence.
    pub pipeline: Duration,
}

/// What one retirement attempt observed, per owner.
///
/// [`Self::is_quiescent`] is the reap's precondition: false means at least one
/// owner can still be writing for the streamer, so the row has to stay.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct StreamerRetirement {
    /// Owners that had not acknowledged when their bound expired, named for the
    /// log line the caller writes.
    pub outstanding: Vec<String>,
}

impl StreamerRetirement {
    /// Whether every owner acknowledged, which is what lets the caller reap.
    pub fn is_quiescent(&self) -> bool {
        self.outstanding.is_empty()
    }
}

impl RuntimeCoordinator {
    /// Wait until nothing is working on `streamer_id` any more.
    ///
    /// Assumes the row already carries `deleted_at`, so
    /// `Scheduler::ensure_streamer_actor_state` and
    /// `run_live_download_pipeline` refuse to start anything new for it; this
    /// only retires what was already in flight. Safe to call repeatedly: every
    /// step is a no-op once its owner has stopped, which is what lets
    /// `ServiceContainer::spawn_streamer_reaper` re-attempt a retirement that
    /// did not finish.
    ///
    /// `streamer_name` comes from the marked `streamers` row rather than from a
    /// session lookup: `SessionLifecycle::end_for_disable` puts it on the
    /// `SessionTransition::Ended` broadcast that the notification service reads.
    pub(crate) async fn retire_streamer(
        &self,
        streamer_id: &str,
        streamer_name: &str,
        bounds: RetirementBounds,
    ) -> StreamerRetirement {
        let mut retirement = StreamerRetirement::default();

        // The actor first: while it runs it can still observe the streamer live
        // and publish `MonitorEvent::StreamerLive`, and although
        // `run_live_download_pipeline` drops that on the marked metadata's
        // `is_active`, the check itself writes `streamer_check_history` rows
        // that the reap's cascade would remove underneath it.
        match self
            .scheduler_handle
            .remove_streamer_awaitable(streamer_id)
            .await
        {
            Some(removal) => match removal.wait(bounds.actor).await {
                ActorRemovalOutcome::NotRegistered | ActorRemovalOutcome::Stopped => {
                    debug!(streamer_id, "Streamer actor stopped");
                }
                ActorRemovalOutcome::TimedOut => {
                    warn!(
                        streamer_id,
                        generations = ?removal.tracked_generations(),
                        "Streamer actor still running at the retirement bound"
                    );
                    retirement.outstanding.push("scheduler actor".to_string());
                }
            },
            // The event loop is not running, so nothing observed the removal.
            // Reading that as "stopped" would let the reap delete the row out
            // from under a live actor.
            None => {
                debug!(
                    streamer_id,
                    "Scheduler event loop is not running; actor removal unobserved"
                );
                retirement.outstanding.push("scheduler actor".to_string());
            }
        }

        // Cancels the queued attempts and current session token, awaits every
        // running attempt's terminal outcome, stops danmu collection, and ends
        // the session through `SessionLifecycle` so the `session_ended` audit
        // row and the `SessionTransition::Ended` broadcast still happen.
        let stop = self
            .stop_streamer_work(
                streamer_id,
                streamer_name,
                StopWait::Finalized {
                    bound: bounds.download,
                },
            )
            .await;
        retirement.outstanding.extend(stop.failures);

        if let Some(session_id) = stop.session_id {
            debug!(
                streamer_id,
                session_id, "Session stop observed; waiting for its durable pipeline receipt"
            );
        }
        match self
            .pipeline_manager
            .drain_for_streamer(streamer_id, bounds.pipeline)
            .await
        {
            Ok(drain) if drain.is_drained() => debug!(streamer_id, "All session pipelines settled"),
            Ok(drain) => {
                info!(streamer_id, sessions = ?drain.sessions, jobs = drain.jobs, "Recording post-processing still prevents streamer removal");
                retirement.outstanding.push(format!(
                    "pipeline work: {} unsettled sessions, {} jobs",
                    drain.sessions.len(),
                    drain.jobs
                ));
            }
            Err(error) => {
                warn!(streamer_id, %error, "Failed to observe streamer pipeline settlement");
                retirement
                    .outstanding
                    .push(format!("pipeline settlement: {error}"));
            }
        }

        retirement
    }

    /// Mark, retire and reap `streamer_id` in one call.
    ///
    /// The single path behind `DELETE /api/streamers/{id}`,
    /// `BatchStreamerAction::Delete` and the MCP `streamer_delete` tool. The
    /// `deleted_at` write is what the user's request durably buys: it is
    /// committed before anything is awaited, and a retirement that does not
    /// finish inside `bounds` leaves the row for
    /// `ServiceContainer::spawn_streamer_reaper` rather than failing the call.
    ///
    /// `Ok(false)` when no row was marked, which is either an unknown streamer
    /// or one whose retirement another caller already owns.
    pub(crate) async fn delete_streamer(
        &self,
        streamer_id: &str,
        bounds: RetirementBounds,
    ) -> crate::Result<bool> {
        let Some(marked) = self.streamer_manager.mark_deleting(streamer_id).await? else {
            return Ok(false);
        };

        self.retire_and_reap(streamer_id, &marked.name, bounds)
            .await?;
        Ok(true)
    }

    /// Retire a streamer already carrying a `deleted_at` marker and reap it if
    /// every owner acknowledged.
    ///
    /// Returns whether the row was removed; `false` means the marker survives
    /// and the reaper sweep will try again.
    pub(crate) async fn retire_and_reap(
        &self,
        streamer_id: &str,
        streamer_name: &str,
        bounds: RetirementBounds,
    ) -> crate::Result<bool> {
        let retirement = self
            .retire_streamer(streamer_id, streamer_name, bounds)
            .await;

        if !retirement.is_quiescent() {
            info!(
                streamer_id,
                outstanding = %retirement.outstanding.join("; "),
                "Streamer stays marked deleted until its remaining work finishes"
            );
            return Ok(false);
        }

        self.streamer_manager.reap_deleted(streamer_id).await
    }

    /// Retire and reap every row left carrying a `deleted_at` marker.
    ///
    /// Runs at startup — where the marked rows are the ones a crash left between
    /// the mark and the reap — and on the reaper's periodic sweep, where they
    /// are the ones whose post-processing outlived an interactive delete.
    /// Reports how many rows it removed.
    pub(crate) async fn reap_marked_streamers(&self, bounds: RetirementBounds) -> usize {
        let mut reaped = 0;
        for metadata in self.streamer_manager.get_pending_deletion() {
            match self
                .retire_and_reap(&metadata.id, &metadata.name, bounds)
                .await
            {
                Ok(true) => reaped += 1,
                Ok(false) => {}
                Err(error) => warn!(
                    streamer_id = %metadata.id,
                    error = %error,
                    "Failed to reap a deleted streamer"
                ),
            }
        }
        reaped
    }
}
