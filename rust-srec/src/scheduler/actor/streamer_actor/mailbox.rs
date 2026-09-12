use super::*;

impl StreamerActor {
    /// Handle an incoming message.
    ///
    /// Returns `true` if the actor should stop.
    pub(super) async fn handle_message(
        &mut self,
        msg: StreamerMessage,
    ) -> Result<bool, ActorError> {
        match msg {
            StreamerMessage::LifecycleFeedback(envelope) => {
                use crate::scheduler::feedback::FeedbackDisposition;
                let result = if self.cancellation_token.is_cancelled() {
                    Ok(FeedbackDisposition::Retired)
                } else if envelope.sequence <= self.feedback_sequence
                    || envelope
                        .current_epoch
                        .load(std::sync::atomic::Ordering::Acquire)
                        != envelope.epoch
                {
                    Ok(FeedbackDisposition::Superseded)
                } else {
                    self.apply_lifecycle_feedback(
                        envelope.sequence,
                        envelope.event.as_ref().clone(),
                    )
                    .await
                };
                let reply = match &result {
                    Ok(disposition) => Ok(*disposition),
                    Err(error) if !error.recoverable => Ok(FeedbackDisposition::Retired),
                    Err(error) => Err(error.to_string()),
                };
                drop(envelope.lease);
                let _ = envelope.applied.send(reply);
                result?;
                Ok(false)
            }
            StreamerMessage::RetainedConfig { config, applied } => {
                if config.revision > self.config_revision {
                    self.handle_config_update(config.config).await?;
                    self.config_revision = config.revision;
                }
                let _ = applied.send(());
                Ok(false)
            }
            StreamerMessage::RetryAdmission => {
                if self.current_download.is_none() {
                    self.state.streamer_state = StreamerState::NotLive;
                    self.state.schedule_immediate_check();
                }
                Ok(false)
            }
            StreamerMessage::CheckStatus => {
                self.handle_check_status().await?;
                Ok(false)
            }
            StreamerMessage::ConfigUpdate(config) => {
                self.handle_config_update(config).await?;
                Ok(false)
            }
            StreamerMessage::BatchResult(result) => {
                self.handle_batch_result(*result).await?;
                Ok(false)
            }
            StreamerMessage::DownloadStarted {
                download_id,
                session_id,
            } => {
                self.handle_download_started(download_id, session_id);
                Ok(false)
            }
            StreamerMessage::DownloadHeartbeat {
                download_id,
                session_id,
                progress,
            } => {
                self.handle_download_heartbeat(download_id, session_id, progress);
                Ok(false)
            }
            StreamerMessage::DownloadEnded(reason) => {
                self.current_download = None;
                self.handle_download_ended(reason).await?;
                Ok(false)
            }
            StreamerMessage::Stop => {
                self.handle_stop().await?;
                Ok(true)
            }
            StreamerMessage::GetState(reply) => {
                self.handle_get_state(reply).await;
                Ok(false)
            }
        }
    }

    /// Handle CheckStatus message - trigger an immediate check.
    pub(super) async fn handle_check_status(&mut self) -> Result<(), ActorError> {
        debug!("StreamerActor {} received CheckStatus", self.id);

        // Reset next check to now to trigger immediate check
        self.state.set_next_check(Some(Instant::now()));

        Ok(())
    }

    /// Handle ConfigUpdate message - apply new configuration without restart.
    ///
    /// Changed cadence can bring recurring polling forward, but does not postpone
    /// existing work or advance an admission/backoff/smart-wake deadline.
    pub(super) async fn handle_config_update(
        &mut self,
        config: StreamerConfig,
    ) -> Result<(), ActorError> {
        debug!("StreamerActor {} received ConfigUpdate", self.id);

        let old_config = std::mem::replace(&mut self.config, config);

        // Log significant changes
        if old_config.check_interval_ms != self.config.check_interval_ms {
            info!(
                "StreamerActor {} check interval changed: {}ms -> {}ms",
                self.id, old_config.check_interval_ms, self.config.check_interval_ms
            );
        }

        if old_config.priority != self.config.priority {
            info!(
                "StreamerActor {} priority changed: {:?} -> {:?}",
                self.id, old_config.priority, self.config.priority
            );
        }

        self.state
            .reschedule_for_config(&self.config, self.get_error_count());

        Ok(())
    }

    /// Handle Stop message - prepare for graceful shutdown.
    pub(super) async fn handle_stop(&mut self) -> Result<(), ActorError> {
        info!("StreamerActor {} received Stop", self.id);

        Ok(())
    }

    /// Handle GetState message - return current state via oneshot channel.
    pub(super) async fn handle_get_state(
        &self,
        reply: tokio::sync::oneshot::Sender<StreamerActorState>,
    ) {
        debug!("StreamerActor {} received GetState", self.id);

        // Send state, ignore if receiver dropped
        let _ = reply.send(self.state.clone());
    }
}
