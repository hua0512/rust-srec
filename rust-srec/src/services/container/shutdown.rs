//! Shutdown deadlines and ordered service containment.

use std::time::Duration;

use tracing::{debug, info, warn};

use crate::Result;

use super::ServiceContainer;

/// Default shutdown timeout.
const DEFAULT_SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(30);

/// Floor for the window `shutdown_until` hands `TaskSupervisor::shutdown`.
///
/// The producer drains run first against the same absolute deadline, so a slow
/// recording flush can leave nothing behind. Without a floor every supervised
/// task that merely had not been reaped yet would be reported as an overrun.
const BACKGROUND_TASK_MIN_GRACE_PERIOD: Duration = Duration::from_secs(2);

/// Time reserved inside a hard cap for aborted tasks to settle. Joining an
/// aborted task proves its future was dropped, which is when an engine child
/// spawned with `kill_on_drop` is killed.
const ABORT_REAP_WINDOW: Duration = Duration::from_secs(5);

/// Absolute deadlines consumed by the service shutdown module.
///
/// The cooperative drain may keep containing owned work after
/// `cooperative_deadline`, but at `force_deadline` it is dropped and the
/// remaining supervised tasks are aborted. All abort/reap work shares
/// `hard_deadline`; no phase extends the caller's hard bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ServiceShutdownSchedule {
    cooperative_deadline: tokio::time::Instant,
    force_deadline: tokio::time::Instant,
    hard_deadline: tokio::time::Instant,
}

impl ServiceShutdownSchedule {
    pub(crate) fn new(
        cooperative_deadline: tokio::time::Instant,
        force_deadline: tokio::time::Instant,
        hard_deadline: tokio::time::Instant,
    ) -> Result<Self> {
        if cooperative_deadline > force_deadline || force_deadline > hard_deadline {
            return Err(crate::Error::validation(
                "service shutdown deadlines must satisfy cooperative <= force <= hard",
            ));
        }
        Ok(Self {
            cooperative_deadline,
            force_deadline,
            hard_deadline,
        })
    }

    fn from_now(grace_period: Duration, hard_cap: Duration) -> Result<Self> {
        let started_at = tokio::time::Instant::now();
        let hard_deadline = started_at.checked_add(hard_cap).ok_or_else(|| {
            crate::Error::validation("service shutdown hard cap exceeds the monotonic clock range")
        })?;
        // A fixed five-second reserve would consume an entire short cap and
        // force even quiescent services immediately. Preserve at least half of
        // a short cap for cooperative shutdown while retaining the full reap
        // window for normal production-sized caps.
        let abort_reap_window = ABORT_REAP_WINDOW.min(hard_cap / 2);
        let force_deadline = hard_deadline
            .checked_sub(abort_reap_window)
            .unwrap_or(started_at)
            .max(started_at);
        let cooperative_deadline = started_at
            .checked_add(grace_period)
            .unwrap_or(force_deadline)
            .min(force_deadline);
        Self::new(cooperative_deadline, force_deadline, hard_deadline)
    }
}

impl ServiceContainer {
    /// Shutdown all services, waiting as long as containment takes.
    ///
    /// `DEFAULT_SHUTDOWN_GRACE_PERIOD` bounds only the cooperative
    /// phase. The drains in `TaskSupervisor::shutdown`,
    /// `DownloadManager::shutdown_until` and `PipelineManager::stop` all keep
    /// joining past it so no producer outlives the database pools, so this call
    /// has no wall-clock bound and a wedged engine holds it open indefinitely.
    /// Only use it under a parent process that force-kills the process tree;
    /// in-process embedders want [`Self::shutdown_with_hard_cap`].
    pub async fn shutdown(&self) -> Result<()> {
        self.shutdown_with_grace_period(DEFAULT_SHUTDOWN_GRACE_PERIOD)
            .await
    }

    /// Shutdown all services within `hard_cap`.
    ///
    /// [`Self::shutdown`] waits as long as containment takes: the drains in
    /// `TaskSupervisor::shutdown`, `DownloadManager::shutdown_until` and
    /// `PipelineManager::stop` all keep joining past the grace period so no
    /// producer outlives the database pools. That is only safe with a parent
    /// process that enforces a wall-clock deadline and force-kills the process
    /// tree. Embedders that run the container in-process call
    /// this instead: before `hard_cap` expires the phased drain is dropped and
    /// the remaining work is aborted, so each attempt/job future is dropped and
    /// the ffmpeg/streamlink child it owns through `kill_on_drop` is killed
    /// rather than orphaned by a later `std::process::exit`.
    ///
    /// Returns `Err` naming what was still running when the cap fired. On that
    /// path the database pools are left open, matching the quiescence gate in
    /// the cooperative shutdown path; the caller is expected to exit the
    /// process. `ABORT_REAP_WINDOW` is reserved inside `hard_cap`, so this
    /// method does not add a second timeout after the caller's deadline.
    pub async fn shutdown_with_hard_cap(
        &self,
        grace_period: Duration,
        hard_cap: Duration,
    ) -> Result<()> {
        let schedule = ServiceShutdownSchedule::from_now(grace_period, hard_cap)?;
        self.shutdown_with_schedule(schedule).await
    }

    /// Run the phased shutdown against caller-owned absolute deadlines.
    pub(crate) async fn shutdown_with_schedule(
        &self,
        schedule: ServiceShutdownSchedule,
    ) -> Result<()> {
        let mut graceful = Box::pin(self.shutdown_until(schedule.cooperative_deadline));
        if let Ok(result) = tokio::time::timeout_at(schedule.force_deadline, &mut graceful).await {
            return result;
        }

        // Drop the drain before aborting. Every drain it owns hands its
        // `JoinSet` back to its service through `DrainedTasks` when its future
        // is dropped, which is what lets the abort hatches below reach the
        // tasks that are still running instead of silently aborting them here.
        drop(graceful);
        warn!(
            "Service shutdown reached its force deadline; aborting the remaining supervised work"
        );

        let (
            aborted_downloads,
            aborted_collections,
            aborted_timers,
            aborted_pipeline_tasks,
            aborted_background_tasks,
        ) = tokio::join!(
            self.download_manager.abort_attempts(schedule.hard_deadline),
            self.danmu_service.abort_collections(schedule.hard_deadline),
            self.session_lifecycle.abort_timers(schedule.hard_deadline),
            self.pipeline_manager.abort(schedule.hard_deadline),
            self.task_supervisor.abort_all(schedule.hard_deadline),
        );

        warn!(
            downloads = ?aborted_downloads,
            collections = ?aborted_collections,
            hysteresis_timers = aborted_timers,
            pipeline_tasks = aborted_pipeline_tasks,
            background_tasks = aborted_background_tasks,
            "Aborted supervised work after the shutdown force deadline"
        );

        Err(crate::Error::Other(format!(
            "service shutdown exceeded its force deadline: aborted {} recording attempt(s) {aborted_downloads:?}, {} danmu collection(s) {aborted_collections:?}, {aborted_timers} hysteresis timer(s), {aborted_pipeline_tasks} pipeline task(s) and {aborted_background_tasks} background task(s) by the hard deadline; database pools were left open",
            aborted_downloads.len(),
            aborted_collections.len()
        )))
    }

    /// Shutdown all services with a bounded cooperative grace period.
    ///
    /// If that period expires, containment may run longer while owned tasks
    /// cancel and join. Database pools are never closed until every producer
    /// that can write through them is proven quiescent.
    pub(crate) async fn shutdown_with_grace_period(&self, grace_period: Duration) -> Result<()> {
        info!("Shutting down services (grace period: {:?})", grace_period);
        let deadline = tokio::time::Instant::now() + grace_period;
        self.shutdown_until(deadline).await
    }

    async fn shutdown_until(&self, deadline: tokio::time::Instant) -> Result<()> {
        // Containment failures: something could not be proven to have stopped
        // writing. Only these make this method return `Err`, which `server.rs`
        // turns into a nonzero worker exit and `classify_settled_exit` records
        // as a crash that retains a dirty generation marker.
        let mut failures = Vec::new();
        // Soft-budget overruns and errors from work that ended earlier in the
        // run. Both are reported and then ignored: a drain that overran its
        // grace period still finalized every recording, and a download that
        // failed hours ago says nothing about this shutdown.
        let mut overruns = Vec::new();
        let mut runtime_failures = Vec::new();
        let mut coordination_drained = true;

        // Fence producers before cancellation. Required coordination consumers
        // are marker-driven, so they remain alive long enough to persist every
        // final segment and terminal fact after auxiliary loops stop.
        self.stream_monitor.stop();
        self.download_manager.shutdown_queue();
        self.cancellation_token.cancel();

        info!("Stopping download manager...");
        let download_report = self.download_manager.shutdown_until(deadline).await;
        info!(
            count = download_report.stopped_download_ids.len(),
            deadline_exceeded = download_report.deadline_exceeded_download_ids.len(),
            "Stopped active downloads and drained required download events"
        );
        coordination_drained &= download_report.coordination_drained;
        runtime_failures.extend(
            download_report
                .runtime_failures
                .into_iter()
                .map(|failure| format!("download run: {failure}")),
        );
        overruns.extend(
            download_report
                .overruns
                .into_iter()
                .map(|overrun| format!("download shutdown: {overrun}")),
        );
        failures.extend(
            download_report
                .failures
                .into_iter()
                .map(|failure| format!("download shutdown: {failure}")),
        );

        info!("Stopping danmu service...");
        let danmu_report = self.danmu_service.shutdown_until(deadline).await;
        if !danmu_report.forced_session_ids.is_empty() {
            warn!(
                sessions = ?danmu_report.forced_session_ids,
                "Danmu collections exceeded the graceful deadline and were drained"
            );
        }
        runtime_failures.extend(
            danmu_report
                .runtime_failures
                .into_iter()
                .map(|failure| format!("danmu run: {failure}")),
        );
        overruns.extend(
            danmu_report
                .overruns
                .into_iter()
                .map(|overrun| format!("danmu shutdown: {overrun}")),
        );
        failures.extend(
            danmu_report
                .shutdown_failures
                .into_iter()
                .map(|failure| format!("danmu shutdown: {failure}")),
        );

        if self.danmu_coordination_receiver.lock().is_none() {
            match tokio::time::timeout_at(deadline, self.danmu_coordination_sender.shutdown()).await
            {
                Ok(Ok(event_failures)) => {
                    // Per-event handler errors accumulated across the whole run,
                    // not shutdown-phase containment failures.
                    runtime_failures.extend(
                        event_failures
                            .into_iter()
                            .map(|failure| format!("danmu coordination event failed: {failure}")),
                    );
                }
                Ok(Err(error)) => {
                    let message = format!("failed to drain danmu coordination events: {error}");
                    warn!(%message);
                    failures.push(message);
                    coordination_drained = false;
                }
                Err(_) => {
                    let message = "danmu coordination drain deadline exceeded".to_string();
                    warn!(%message);
                    failures.push(message);
                    coordination_drained = false;
                }
            }
        } else {
            debug!("Danmu coordination handler was not started; skipping its shutdown barrier");
        }

        let lifecycle_report = self.session_lifecycle.shutdown_until(deadline).await;
        if lifecycle_report.forced_timer_count > 0 {
            warn!(
                count = lifecycle_report.forced_timer_count,
                "Session hysteresis timers exceeded the graceful deadline and were drained"
            );
        }
        overruns.extend(
            lifecycle_report
                .overruns
                .into_iter()
                .map(|overrun| format!("session lifecycle: {overrun}")),
        );
        failures.extend(
            lifecycle_report
                .failures
                .into_iter()
                .map(|failure| format!("session lifecycle task failed: {failure}")),
        );

        if self.session_transition_receiver.lock().is_none() {
            match tokio::time::timeout_at(deadline, self.session_transition_sender.shutdown()).await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    let message = format!("failed to drain session transitions: {error}");
                    warn!(%message);
                    failures.push(message);
                    coordination_drained = false;
                }
                Err(_) => {
                    let message = "session transition drain deadline exceeded".to_string();
                    warn!(%message);
                    failures.push(message);
                    coordination_drained = false;
                }
            }
        } else {
            debug!("Session transition coordinator was not started; skipping its shutdown barrier");
        }

        // Every producer above joins past `deadline` rather than abandoning
        // work, so reaching here means they are quiesced regardless of whether
        // they stayed inside their budget. Downstream services can stop.
        info!("Stopping pipeline manager...");
        self.pipeline_manager.stop().await;
        info!("Stopping notification service...");
        self.notification_service.stop().await;

        // The producer drains above may have consumed the whole window. Give
        // the background tasks their own grace period rather than a zero-length
        // one that reports an overrun for every task that is merely unreaped.
        let remaining = deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .max(BACKGROUND_TASK_MIN_GRACE_PERIOD);
        if !self.task_supervisor.shutdown(remaining).await {
            let message =
                "one or more background tasks exceeded the graceful shutdown period".to_string();
            warn!(%message);
            overruns.push(message);
        }

        // `TaskSupervisor::shutdown` has joined the required consumers by now,
        // so nothing can still be mid-write. An undrained marker means one of
        // them stopped without acknowledging, and leaving the pools open keeps
        // the last observed state visible to the operator who has to reconcile
        // it — the process exits immediately after this either way.
        if coordination_drained {
            info!("Closing database pools...");
            tokio::join!(self.write_pool.close(), self.pool.close());
        } else {
            warn!("Required coordination markers did not drain; leaving database pools open");
        }

        for failure in &runtime_failures {
            warn!(%failure, "Work ended with an error during the run");
        }
        for overrun in &overruns {
            warn!(%overrun, "Shutdown phase exceeded its grace period but was contained");
        }

        info!("Services shut down");
        if failures.is_empty() {
            Ok(())
        } else {
            Err(crate::Error::Other(format!(
                "service shutdown incomplete: {}",
                failures.join("; ")
            )))
        }
    }
}
