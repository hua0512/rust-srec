use super::*;

impl StreamerActor {
    pub(super) async fn apply_lifecycle_feedback(
        &mut self,
        sequence: u64,
        event: crate::downloader::DownloadManagerEvent,
    ) -> Result<crate::scheduler::feedback::FeedbackDisposition, ActorError> {
        use crate::downloader::{
            DownloadManagerEvent, DownloadProgressEvent, DownloadTerminalEvent,
        };
        use crate::scheduler::feedback::FeedbackDisposition;
        match event {
            DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadStarted {
                download_id,
                session_id,
                ..
            }) => {
                self.current_download = Some((download_id.clone(), session_id.clone()));
                self.handle_download_started(download_id, session_id)
                    .await?;
            }
            DownloadManagerEvent::Terminal(terminal) => {
                if self
                    .current_download
                    .as_ref()
                    .is_some_and(|(download, session)| {
                        terminal.download_id() != Some(download.as_str())
                            || terminal.session_id() != session
                    })
                {
                    self.feedback_sequence = sequence;
                    return Ok(FeedbackDisposition::Superseded);
                }
                let policy = match terminal {
                    DownloadTerminalEvent::Completed { stop_cause, .. } => stop_cause
                        .map(crate::scheduler::service::download_end_policy_for_stop)
                        .unwrap_or(DownloadEndPolicy::Completed),
                    DownloadTerminalEvent::Cancelled { cause, .. } => {
                        crate::scheduler::service::download_end_policy_for_stop(cause)
                    }
                    DownloadTerminalEvent::Failed { error, .. } => {
                        DownloadEndPolicy::SegmentFailed(error)
                    }
                    DownloadTerminalEvent::Rejected {
                        reason,
                        retry_after_secs,
                        session_id,
                        kind,
                        ..
                    } => {
                        let retry_after_secs = retry_after_secs.unwrap_or(60);
                        match kind {
                            crate::downloader::DownloadRejectedKind::CircuitBreaker => {
                                DownloadEndPolicy::CircuitBreakerBlocked {
                                    reason,
                                    retry_after_secs,
                                    session_id,
                                }
                            }
                            crate::downloader::DownloadRejectedKind::OutputRootUnavailable {
                                path,
                                io_kind,
                            } => DownloadEndPolicy::OutputRootBlocked {
                                path,
                                io_kind,
                                retry_after_secs,
                                session_id,
                            },
                            crate::downloader::DownloadRejectedKind::StreamerBackoff => {
                                DownloadEndPolicy::StreamerBackoffBlocked {
                                    reason,
                                    retry_after_secs,
                                    session_id,
                                }
                            }
                        }
                    }
                };
                self.current_download = None;
                self.handle_download_ended(policy).await?;
            }
            _ => return Ok(FeedbackDisposition::Superseded),
        }
        self.feedback_sequence = sequence;
        Ok(FeedbackDisposition::Applied)
    }

    /// Handle DownloadStarted message - pause status checking while a download is active.
    pub(super) async fn handle_download_started(
        &mut self,
        download_id: String,
        session_id: String,
    ) -> Result<(), ActorError> {
        info!(
            "StreamerActor {} download started: download_id={}, session_id={}",
            self.id, download_id, session_id
        );

        self.seed_active_download(download_id, session_id);

        Ok(())
    }

    /// Initialize a replacement from the recording owner's current identity.
    /// This is a snapshot, not a replay of completed session side effects.
    pub(in crate::scheduler::actor) fn seed_active_download(
        &mut self,
        download_id: String,
        session_id: String,
    ) {
        self.current_download = Some((download_id, session_id));

        // Pause checks by switching to Live scheduling behavior.
        // This is primarily for externally orchestrated downloads where the actor
        // might not have just observed a Live check result.
        self.state.streamer_state = StreamerState::Live;
        self.state.hysteresis.mark_live();
        self.state.last_download_activity_at = Some(Instant::now());
        self.state
            .schedule_next_check(&self.config, self.get_error_count());
    }

    pub(super) fn handle_download_heartbeat(
        &mut self,
        download_id: String,
        session_id: String,
        progress: Option<crate::downloader::engine::DownloadProgress>,
    ) {
        if self
            .current_download
            .as_ref()
            .is_none_or(|current| current.0 != download_id || current.1 != session_id)
        {
            return;
        }
        // Heartbeats are intentionally lightweight; they are used only to avoid triggering
        // platform extraction while a download is actively making progress.
        self.state.last_download_activity_at = Some(Instant::now());
        if let Some(progress) = progress {
            trace!(
                "StreamerActor {} download heartbeat: download_id={}, session_id={}, bytes={}, segments={}, speed={}",
                self.id,
                download_id,
                session_id,
                progress.bytes_downloaded,
                progress.segments_completed,
                progress.speed_bytes_per_sec
            );
        } else {
            trace!(
                "StreamerActor {} download heartbeat: download_id={}, session_id={}",
                self.id, download_id, session_id
            );
        }
    }
}
