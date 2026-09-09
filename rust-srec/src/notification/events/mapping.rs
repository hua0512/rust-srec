use std::collections::HashMap;

use chrono::{DateTime, Utc};

use super::NotificationEvent;
use crate::downloader::{DownloadManagerEvent, DownloadProgressEvent, DownloadTerminalEvent};
use crate::monitor::MonitorEvent;
use crate::pipeline::PipelineEvent;
use crate::session::SessionTransition;

pub(crate) fn monitor_notification(event: MonitorEvent) -> Option<NotificationEvent> {
    match event {
        MonitorEvent::FatalError {
            streamer_id,
            streamer_name,
            error_type,
            message,
            timestamp,
            ..
        } => Some(NotificationEvent::FatalError {
            streamer_id,
            streamer_name,
            error_type: format!("{:?}", error_type),
            message,
            timestamp,
        }),
        _ => None,
    }
}

pub(crate) fn session_notification(transition: SessionTransition) -> Option<NotificationEvent> {
    match transition {
        crate::session::SessionTransition::Started {
            streamer_id,
            streamer_name,
            title,
            category,
            started_at,
            ..
        } => Some(NotificationEvent::StreamOnline {
            streamer_id,
            streamer_name,
            title,
            category,
            timestamp: started_at,
        }),
        crate::session::SessionTransition::Ended {
            cause: crate::session::TerminalCause::UserDisabled,
            ..
        } => {
            // User explicitly disabled / deleted the streamer.
            // They know — no need to ping their notification
            // channels with a synthetic offline event.
            None
        }
        crate::session::SessionTransition::Ended {
            cause: crate::session::TerminalCause::OutOfSchedule,
            ..
        } => {
            // The recording schedule closed while the stream may
            // still be live; don't report this as platform offline.
            None
        }
        crate::session::SessionTransition::Ended {
            streamer_id,
            streamer_name,
            ended_at,
            ..
        } => Some(NotificationEvent::StreamOffline {
            streamer_id,
            streamer_name,
            duration_secs: None,
            timestamp: ended_at,
        }),
        // Hysteresis quiet-period transitions are
        // intentionally ignored here. Only the final
        // `Ended` produces a StreamOffline notification.
        // `Resumed` similarly yields no user-facing
        // notification — the original Started already
        // fired and remains valid.
        crate::session::SessionTransition::Ending { .. }
        | crate::session::SessionTransition::Resumed { .. } => None,
    }
}

pub(crate) fn download_notification(
    event: DownloadManagerEvent,
    now: DateTime<Utc>,
) -> Option<NotificationEvent> {
    match event {
        DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadStarted {
            streamer_id,
            streamer_name,
            session_id,
            ..
        }) => Some(NotificationEvent::DownloadStarted {
            streamer_id,
            streamer_name,
            session_id,
            timestamp: now,
        }),
        DownloadManagerEvent::Terminal(DownloadTerminalEvent::Completed {
            streamer_id,
            streamer_name,
            session_id,
            total_bytes,
            total_duration_secs,
            ..
        }) => Some(NotificationEvent::DownloadCompleted {
            streamer_id,
            streamer_name,
            session_id,
            file_size_bytes: total_bytes,
            duration_secs: total_duration_secs,
            timestamp: now,
        }),
        DownloadManagerEvent::Terminal(DownloadTerminalEvent::Failed {
            streamer_id,
            streamer_name,
            error,
            recoverable,
            ..
        }) => Some(NotificationEvent::DownloadError {
            streamer_id,
            streamer_name,
            error_message: error,
            recoverable,
            timestamp: now,
        }),
        DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentStarted {
            streamer_id,
            streamer_name,
            session_id,
            segment_path,
            segment_index,
            started_at,
            ..
        }) => Some(NotificationEvent::SegmentStarted {
            streamer_id,
            streamer_name,
            session_id,
            segment_path,
            segment_index,
            timestamp: started_at,
        }),
        DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentCompleted {
            streamer_id,
            streamer_name,
            session_id,
            segment_path,
            segment_index,
            completed_at,
            duration_secs,
            size_bytes,
            ..
        }) => Some(NotificationEvent::SegmentCompleted {
            streamer_id,
            streamer_name,
            session_id,
            segment_path,
            segment_index,
            size_bytes,
            duration_secs,
            timestamp: completed_at,
        }),
        DownloadManagerEvent::Terminal(DownloadTerminalEvent::Cancelled {
            streamer_id,
            streamer_name,
            session_id,
            ..
        }) => Some(NotificationEvent::DownloadCancelled {
            streamer_id,
            streamer_name,
            session_id,
            timestamp: now,
        }),
        DownloadManagerEvent::Terminal(DownloadTerminalEvent::Rejected {
            streamer_id,
            streamer_name,
            session_id,
            reason,
            ..
        }) => Some(NotificationEvent::DownloadRejected {
            streamer_id,
            streamer_name,
            session_id,
            reason,
            timestamp: now,
        }),
        DownloadManagerEvent::Progress(DownloadProgressEvent::ConfigUpdated {
            streamer_id,
            streamer_name,
            update_type,
            ..
        }) => Some(NotificationEvent::ConfigUpdated {
            streamer_id,
            streamer_name,
            update_type: format!("{:?}", update_type),
            timestamp: now,
        }),
        _ => None,
    }
}

#[derive(Default)]
pub(crate) struct PipelineNotifications {
    job_to_streamer: HashMap<String, String>,
}

impl PipelineNotifications {
    pub(crate) fn map(
        &mut self,
        event: PipelineEvent,
        now: DateTime<Utc>,
    ) -> Option<NotificationEvent> {
        match event {
            PipelineEvent::JobEnqueued {
                job_id,
                streamer_id,
                ..
            } => {
                self.job_to_streamer.insert(job_id, streamer_id);
                None
            }
            PipelineEvent::JobStarted {
                job_id,
                job_type,
                streamer_id,
            } => {
                // DAG step jobs are enqueued through
                // JobQueue::enqueue_existing and never produce a
                // JobEnqueued event, so self.job_to_streamer has no entry
                // for them; the worker's own streamer_id is the
                // reliable source.
                let streamer_id = if streamer_id.is_empty() {
                    self.job_to_streamer
                        .get(&job_id)
                        .cloned()
                        .unwrap_or_default()
                } else {
                    streamer_id
                };
                Some(NotificationEvent::PipelineStarted {
                    job_id,
                    job_type,
                    streamer_id,
                    timestamp: now,
                })
            }
            PipelineEvent::JobCompleted {
                job_id,
                job_type,
                duration_secs,
            } => {
                self.job_to_streamer.remove(&job_id);
                Some(NotificationEvent::PipelineCompleted {
                    job_id,
                    job_type,
                    output_path: None,
                    duration_secs,
                    timestamp: now,
                })
            }
            PipelineEvent::JobFailed {
                job_id,
                job_type,
                error,
            } => {
                self.job_to_streamer.remove(&job_id);
                Some(NotificationEvent::PipelineFailed {
                    job_id,
                    job_type,
                    error_message: error,
                    timestamp: now,
                })
            }
            PipelineEvent::QueueWarning { depth } => {
                Some(NotificationEvent::PipelineQueueWarning {
                    queue_depth: depth,
                    threshold: 100, // TODO: Get from config
                    timestamp: now,
                })
            }
            PipelineEvent::QueueCritical { depth } => {
                Some(NotificationEvent::PipelineQueueCritical {
                    queue_depth: depth,
                    threshold: 200, // TODO: Get from config
                    timestamp: now,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests;
