use super::*;

impl StreamerActor {
    /// Delegate check to the platform actor for batch processing.
    pub(super) async fn delegate_to_platform(&mut self) -> Result<(), ActorError> {
        if let Some(ref platform_actor) = self.platform_actor {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();

            let msg = PlatformMessage::RequestCheck {
                streamer_id: self.id.clone(),
                reply: reply_tx,
            };

            // Send request to platform actor
            if platform_actor.send(msg).await.is_err() {
                return Err(ActorError::recoverable("Platform actor unavailable"));
            }

            // Wait for acknowledgment (not the result - that comes via BatchResult message)
            match tokio::time::timeout(Duration::from_secs(5), reply_rx).await {
                Ok(Ok(())) => {
                    // The result arrives asynchronously via handle_batch_result; park
                    // next_check so the run loop does not re-fire this timer and issue a
                    // duplicate RequestCheck before that result lands.
                    self.state
                        .schedule_next_check(&self.config, self.get_error_count());
                    debug!("StreamerActor {} check delegated to platform", self.id);
                    Ok(())
                }
                Ok(Err(_)) => Err(ActorError::recoverable("Platform actor dropped reply")),
                Err(_) => Err(ActorError::recoverable("Platform actor timeout")),
            }
        } else {
            Err(ActorError::recoverable("No platform actor configured"))
        }
    }

    /// Handle BatchResult message - process result from PlatformActor.
    pub(super) async fn handle_batch_result(
        &mut self,
        result: BatchDetectionResult,
    ) -> Result<(), ActorError> {
        debug!(
            "StreamerActor {} received BatchResult: {:?}",
            self.id, result.result.state
        );

        // Verify this result is for us
        if result.streamer_id != self.id {
            warn!(
                "StreamerActor {} received BatchResult for wrong streamer: {}",
                self.id, result.streamer_id
            );
            return Ok(());
        }

        let previous_runtime_state = self.state.clone();
        let next_state = result.result.state;
        let error_count = self.get_error_count();
        let is_error = result.result.is_error();
        let error_message = result.result.error.clone();

        // Batch failures are handled as errors, not as offline transitions.
        // This matches perform_check() behavior and avoids incorrectly ending sessions.
        if is_error {
            let is_live_watchdog = self.is_live_watchdog();
            if is_live_watchdog {
                let msg = error_message.as_deref().unwrap_or("Batch check failed");
                warn!(
                    "StreamerActor {} live watchdog batch check failed (ignored while download active): {}",
                    self.id, msg
                );
                return Ok(());
            }

            // Record the check result so scheduling/backoff can proceed normally when not Live.
            self.state
                .record_check(result.result, &self.config, error_count);

            if let Some(metadata) = self.get_metadata() {
                let msg = error_message.as_deref().unwrap_or("Batch check failed");
                if let Err(e) = self.status_checker.handle_error(&metadata, msg).await {
                    warn!(
                        "StreamerActor {} failed to handle batch error: {}",
                        self.id, e
                    );
                }
            }
            return Ok(());
        }

        // Record the check result and get hysteresis decision.
        // Reconciliation must precede the Live-result refresh below.
        let reconcile_offline = self.initial_status_pending
            && matches!(result.status, LiveStatus::Offline)
            && !self.state.hysteresis.was_live();
        let settles_initial_status =
            matches!(result.status, LiveStatus::Live { .. } | LiveStatus::Offline);
        self.force_live_reemit_if_stalled(next_state, "batch");

        if next_state == StreamerState::Live {
            self.state.last_download_activity_at = Some(Instant::now());
        }

        let should_emit = self
            .state
            .record_check(result.result, &self.config, error_count);

        if should_emit || reconcile_offline {
            // Fetch fresh metadata for process_status
            if let Some(metadata) = self.get_metadata() {
                match self
                    .status_checker
                    .process_status(&metadata, result.status)
                    .await
                {
                    Ok(ProcessStatusResult::Applied) => {
                        if settles_initial_status {
                            self.initial_status_pending = false;
                        }
                    }
                    Ok(ProcessStatusResult::Suppressed(suppression)) => {
                        if next_state == StreamerState::Live {
                            self.handle_suppressed_live_status(suppression, previous_runtime_state);
                        }
                    }
                    Err(e) => {
                        warn!(
                            "StreamerActor {} failed to process batch status: {}",
                            self.id, e
                        );
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
        }

        debug!(
            "StreamerActor {} batch result processed, next check in {:?}",
            self.id,
            self.state.time_until_next_check()
        );

        Ok(())
    }
}
