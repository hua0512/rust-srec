use super::*;

impl StreamerActor {
    pub(super) fn handle_suppressed_live_status(
        &mut self,
        suppression: ProcessStatusSuppression,
        previous_runtime_state: StreamerActorState,
    ) {
        let retry_after = match suppression {
            ProcessStatusSuppression::Disabled => {
                debug!(
                    streamer_id = %self.id,
                    previous_state = ?previous_runtime_state.streamer_state,
                    "live status suppressed because streamer is manually disabled"
                );
                Duration::from_millis(self.config.check_interval_ms)
            }
            ProcessStatusSuppression::TemporarilyDisabled { retry_after } => {
                let retry_after = retry_after
                    .unwrap_or_else(|| Duration::from_millis(self.config.check_interval_ms));
                debug!(
                    streamer_id = %self.id,
                    previous_state = ?previous_runtime_state.streamer_state,
                    retry_after = ?retry_after,
                    "live status suppressed by temporary disable; reverting actor state"
                );
                retry_after
            }
        };

        self.state = previous_runtime_state;
        self.state.last_download_activity_at = None;
        self.state
            .set_next_check(Instant::now().checked_add(retry_after));
    }

    /// Initiate a status check.
    ///
    /// If on a batch-capable platform, delegates to the PlatformActor.
    /// Otherwise, performs the check directly.
    pub(super) async fn initiate_check(&mut self) -> Result<(), ActorError> {
        // Respect temporary backoff (disabled_until) without removing the actor.
        // This prevents "dead stop" monitoring where the scheduler removes actors
        // for temporary errors and never respawns them.
        let metadata = self
            .get_metadata()
            .ok_or_else(|| ActorError::fatal("Streamer removed from metadata store"))?;
        if metadata.is_disabled() {
            let remaining = metadata.remaining_backoff_std().unwrap_or(Duration::ZERO);

            self.state
                .set_next_check(Instant::now().checked_add(remaining));
            debug!(
                streamer_id = %self.id,
                streamer_name = %metadata.name,
                remaining = ?remaining,
                "status check skipped (backoff)"
            );
            return Ok(());
        }

        debug!(
            streamer_id = %self.id,
            streamer_name = %metadata.name,
            streamer_url = %metadata.url,
            batch = self.uses_batch_detection(),
            "status check start"
        );

        if self.uses_batch_detection() {
            // Delegate to platform actor for batch detection
            self.delegate_to_platform().await?;
        } else {
            // Perform individual check
            self.perform_check().await?;
        }

        Ok(())
    }

    /// Perform an individual status check using the configured status checker.
    ///
    /// This method connects to the actual monitoring infrastructure via the
    /// StatusChecker trait, which abstracts the status checking operation.
    pub(super) async fn perform_check(&mut self) -> Result<(), ActorError> {
        // Captured before the check so the result application below cannot
        // change the answer mid-function; see `is_live_watchdog` for why
        // watchdog failures must stay side-effect free.
        let is_live_watchdog = self.is_live_watchdog();

        // Fetch fresh metadata from the store
        let metadata = self
            .get_metadata()
            .ok_or_else(|| ActorError::fatal("Streamer removed from metadata store"))?;

        // Perform the actual status check using the status checker
        match self.status_checker.check_status(&metadata).await {
            Ok((result, status)) => {
                // A successful check clears any watchdog re-check floor set by a prior
                // failed Live watchdog check.
                self.live_watchdog_backoff_until = None;

                let previous_runtime_state = self.state.clone();
                let next_state = result.state;
                let error_count = self.get_error_count();
                let reconcile_offline = self.initial_status_pending
                    && matches!(status, LiveStatus::Offline)
                    && !self.state.hysteresis.was_live();
                let settles_initial_status =
                    matches!(status, LiveStatus::Live { .. } | LiveStatus::Offline);

                self.force_live_reemit_if_stalled(next_state, "check");

                if next_state == StreamerState::Live {
                    self.state.last_download_activity_at = Some(Instant::now());
                }

                // Record the check result and get hysteresis decision
                let should_emit = self.state.record_check(result, &self.config, error_count);

                if should_emit || reconcile_offline {
                    match self.status_checker.process_status(&metadata, status).await {
                        Ok(ProcessStatusResult::Applied) => {
                            if settles_initial_status {
                                self.initial_status_pending = false;
                            }
                        }
                        Ok(ProcessStatusResult::Suppressed(suppression)) => {
                            if next_state == StreamerState::Live {
                                self.handle_suppressed_live_status(
                                    suppression,
                                    previous_runtime_state,
                                );
                            }
                        }
                        Err(e) => {
                            warn!("StreamerActor {} failed to process status: {}", self.id, e);
                            // Revert Live state to prevent the actor from getting stuck in
                            // the watchdog path when no session/download was actually created.
                            if self.state.streamer_state == StreamerState::Live {
                                self.state.streamer_state = StreamerState::NotLive;
                                self.state.last_download_activity_at = None;
                                self.state.schedule_immediate_check();
                            }
                        }
                    }
                }

                // Check for fatal states - actor should stop monitoring
                // Fatal states indicate permanent issues that require manual intervention
                if next_state == StreamerState::NotFound || next_state == StreamerState::FatalError
                {
                    info!(
                        "StreamerActor {} detected fatal state {:?}, stopping",
                        self.id, next_state
                    );
                    return Err(ActorError::fatal(format!(
                        "Streamer entered fatal state: {:?}",
                        next_state
                    )));
                }

                debug!(
                    streamer_id = %self.id,
                    streamer_name = %metadata.name,
                    state = ?self.state.streamer_state,
                    next_check_in = ?self.state.time_until_next_check(),
                    "status check complete"
                );

                Ok(())
            }
            Err(e) => {
                if is_live_watchdog {
                    // Do not call status_checker.handle_error() and do not record an Error state:
                    // this would increment consecutive error counts, potentially set disabled_until,
                    // and would also switch scheduling away from the Live watchdog cadence.
                    // Arm a re-check floor so the Live scheduling branch of `run` does not
                    // busy-retry against the stall timer while the download is unreachable.
                    self.live_watchdog_backoff_until =
                        Some(Instant::now() + self.live_watchdog_error_backoff());
                    return Err(ActorError::recoverable(format!(
                        "Live watchdog status check failed (ignored while download active): {}",
                        e.message
                    )));
                }

                // Handle the error through the status checker
                if let Err(handle_err) = self
                    .status_checker
                    .handle_error(&metadata, &e.message)
                    .await
                {
                    warn!(
                        "StreamerActor {} failed to handle error: {}",
                        self.id, handle_err
                    );
                }

                // Record the error in state
                let error_result = CheckResult::failure(&e.message);
                self.state
                    .record_check(error_result, &self.config, self.get_error_count());

                if e.transient {
                    // Transient errors are recoverable
                    Err(ActorError::recoverable(e.message))
                } else {
                    // Permanent errors are fatal
                    Err(ActorError::fatal(e.message))
                }
            }
        }
    }
}
