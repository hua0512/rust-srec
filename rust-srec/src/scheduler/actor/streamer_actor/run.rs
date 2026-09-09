use super::*;

impl StreamerActor {
    /// Run the actor's event loop.
    ///
    /// This method runs until the actor receives a Stop message or the
    /// cancellation token is triggered.
    ///
    /// # Returns
    ///
    /// Returns `ActorOutcome::Stopped` on graceful shutdown,
    /// `ActorOutcome::Cancelled` if cancelled externally.
    pub async fn run(mut self) -> ActorResult {
        info!("StreamerActor {} starting", self.id);

        // Schedule initial check if not already scheduled
        if self.state.next_check.is_none() {
            self.state
                .schedule_next_check(&self.config, self.get_error_count());
        }

        loop {
            // First, drain all priority messages before processing normal messages
            // This ensures high-priority operations (like Stop) are handled promptly
            if let Some(msg) = self.try_recv_priority() {
                let start = Instant::now();
                let should_stop = self.handle_run_message(msg).await?;
                self.metrics.record_message(start.elapsed());

                if should_stop {
                    debug!(
                        "StreamerActor {} received stop signal from priority channel",
                        self.id
                    );
                    break;
                }
                // Continue to check for more priority messages
                continue;
            }

            let sleep_duration = self.wake_delay();
            let check_timer = Self::create_check_timer(sleep_duration);

            tokio::select! {
                // Bias towards handling messages first
                biased;

                // Check priority mailbox first (if configured)
                Some(msg) = Self::recv_priority_opt(&mut self.priority_mailbox) => {
                    let start = Instant::now();
                    let should_stop = self.handle_run_message(msg).await?;
                    self.metrics.record_message(start.elapsed());

                    if should_stop {
                        debug!("StreamerActor {} received stop signal from priority channel", self.id);
                        break;
                    }
                }

                // Handle normal-priority messages
                Some(msg) = self.mailbox.recv() => {
                    let start = Instant::now();
                    let should_stop = self.handle_run_message(msg).await?;
                    self.metrics.record_message(start.elapsed());

                    if should_stop {
                        debug!("StreamerActor {} received stop signal", self.id);
                        break;
                    }
                }

                // Self-scheduled check timer
                _ = check_timer => {
                    trace!(streamer_id = %self.id, "check timer fired");
                    if let Err(e) = self.initiate_check().await {
                        warn!("StreamerActor {} check failed: {}", self.id, e);
                        self.metrics.record_error();

                        // Fatal (non-recoverable) errors should stop the actor - the scheduler
                        // will receive a StreamerStateSyncedFromDb event and clean up.
                        if !e.recoverable {
                            info!(
                                "StreamerActor {} stopping due to fatal error: {}",
                                self.id, e.message
                            );
                            break;
                        }
                    }
                }

                // Cancellation
                _ = self.cancellation_token.cancelled() => {
                    info!("StreamerActor {} cancelled", self.id);
                    return Ok(ActorOutcome::Cancelled);
                }
            }
        }

        info!("StreamerActor {} stopped gracefully", self.id);
        Ok(ActorOutcome::Stopped)
    }

    /// Fatal mailbox errors are terminal policy decisions, just like fatal timer errors.
    /// Return a stop signal so all mailbox paths reach graceful shutdown.
    pub(super) async fn handle_run_message(
        &mut self,
        message: StreamerMessage,
    ) -> Result<bool, ActorError> {
        match self.handle_message(message).await {
            Err(error) if !error.recoverable => {
                self.metrics.record_error();
                info!(streamer_id = %self.id, error = %error, "Stopping actor after terminal message error");
                Ok(true)
            }
            result => result,
        }
    }

    /// Try to receive a message from the priority mailbox without blocking.
    pub(super) fn try_recv_priority(&mut self) -> Option<StreamerMessage> {
        if let Some(ref mut priority_rx) = self.priority_mailbox {
            priority_rx.try_recv().ok()
        } else {
            None
        }
    }

    /// Helper to receive from an optional priority mailbox.
    /// Returns a future that is pending forever if the mailbox is None.
    pub(super) async fn recv_priority_opt(
        priority_mailbox: &mut Option<mpsc::Receiver<StreamerMessage>>,
    ) -> Option<StreamerMessage> {
        match priority_mailbox {
            Some(rx) => rx.recv().await,
            None => std::future::pending().await,
        }
    }

    /// Create a future that completes when the next check is due.
    ///
    /// This implements self-scheduling by calculating the delay until
    /// the next check based on the actor's internal state.
    ///
    /// If `duration` is `None`, the timer never fires (waits forever).
    /// This happens when the streamer is live and no check is scheduled.
    pub(super) async fn create_check_timer(duration: Option<Duration>) {
        match duration {
            None => {
                // No check scheduled - wait forever (will be interrupted by other events)
                std::future::pending::<()>().await;
            }
            Some(d) if d.is_zero() => {
                // Check is due immediately, but yield to allow message processing
                tokio::task::yield_now().await;
            }
            Some(d) => {
                tokio::time::sleep(d).await;
            }
        }
    }
}
