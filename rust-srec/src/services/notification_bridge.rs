//! Translates runtime download and pipeline broadcast events into
//! [`NotificationEvent`]s and wires them into [`NotificationService`].

use std::collections::HashMap;
use std::sync::Arc;

use jiff::Timestamp;
use tokio::sync::broadcast;

use crate::downloader::{DownloadManagerEvent, DownloadProgressEvent, DownloadTerminalEvent};
use crate::notification::{NotificationEvent, NotificationService};
use crate::pipeline::PipelineEvent;

/// Start supervised broadcast listeners forwarding download and pipeline
/// events into `notification_service`.
pub(crate) fn start_runtime_notification_listeners(
    notification_service: &Arc<NotificationService>,
    download_rx: broadcast::Receiver<DownloadManagerEvent>,
    pipeline_rx: broadcast::Receiver<PipelineEvent>,
) {
    notification_service.spawn_event_listener(
        "download notification listener",
        download_rx,
        download_notification,
    );

    let mut pipeline_mapper = PipelineNotifications::default();
    notification_service.spawn_event_listener(
        "pipeline notification listener",
        pipeline_rx,
        move |event, now| pipeline_mapper.map(event, now),
    );
}

pub(crate) fn download_notification(
    event: DownloadManagerEvent,
    now: Timestamp,
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
        now: Timestamp,
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
            PipelineEvent::JobRetryScheduled { job_id } => {
                // Drop cached enqueue metadata while the job waits in backoff so
                // cancelling or deleting it never leaks entries; the next retry
                // re-emits JobEnqueued before starting.
                self.job_to_streamer.remove(&job_id);
                None
            }
            PipelineEvent::QueueWarning { depth, threshold } => {
                Some(NotificationEvent::PipelineQueueWarning {
                    queue_depth: depth,
                    threshold,
                    timestamp: now,
                })
            }
            PipelineEvent::QueueCritical { depth, threshold } => {
                Some(NotificationEvent::PipelineQueueCritical {
                    queue_depth: depth,
                    threshold,
                    timestamp: now,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> jiff::Timestamp {
        jiff::Timestamp::from_millisecond(1_788_784_496_123).unwrap()
    }

    #[test]
    fn pipeline_mapping_prefers_explicit_identity_and_forgets_each_terminal_job() {
        let mut mapper = PipelineNotifications::default();
        let source = now();
        for failed in [false, true] {
            assert!(
                mapper
                    .map(
                        PipelineEvent::JobEnqueued {
                            job_id: "job".into(),
                            job_type: "remux".into(),
                            streamer_id: "queued-owner".into()
                        },
                        source
                    )
                    .is_none()
            );
            for (owner, expected) in [("", "queued-owner"), ("direct-owner", "direct-owner")] {
                let event = mapper
                    .map(
                        PipelineEvent::JobStarted {
                            job_id: "job".into(),
                            job_type: "remux".into(),
                            streamer_id: owner.into(),
                        },
                        source,
                    )
                    .unwrap();
                assert!(
                    matches!(event, NotificationEvent::PipelineStarted { streamer_id, timestamp, .. } if streamer_id == expected && timestamp == source)
                );
            }
            let terminal = if failed {
                PipelineEvent::JobFailed {
                    job_id: "job".into(),
                    job_type: "remux".into(),
                    error: "failure".into(),
                }
            } else {
                PipelineEvent::JobCompleted {
                    job_id: "job".into(),
                    job_type: "remux".into(),
                    duration_secs: 12.5,
                }
            };
            let event = mapper.map(terminal, source).unwrap();
            assert_eq!(
                event.event_type(),
                if failed {
                    "pipeline_failed"
                } else {
                    "pipeline_completed"
                }
            );
            assert_eq!(event.timestamp(), source);
            let late = mapper
                .map(
                    PipelineEvent::JobStarted {
                        job_id: "job".into(),
                        job_type: "remux".into(),
                        streamer_id: String::new(),
                    },
                    source,
                )
                .unwrap();
            assert!(
                matches!(late, NotificationEvent::PipelineStarted { streamer_id, .. } if streamer_id.is_empty())
            );
        }
        assert!(
            matches!(mapper.map(PipelineEvent::QueueWarning { depth: 123, threshold: 120 }, source), Some(NotificationEvent::PipelineQueueWarning { queue_depth: 123, threshold: 120, timestamp }) if timestamp == source)
        );
        assert!(
            matches!(mapper.map(PipelineEvent::QueueCritical { depth: 234, threshold: 230 }, source), Some(NotificationEvent::PipelineQueueCritical { queue_depth: 234, threshold: 230, timestamp }) if timestamp == source)
        );

        // Scheduled retries retire the enqueued-streamer cache without emitting a notification.
        assert!(
            mapper
                .map(
                    PipelineEvent::JobEnqueued {
                        job_id: "retrying-job".into(),
                        job_type: "rclone".into(),
                        streamer_id: "owner".into(),
                    },
                    source
                )
                .is_none()
        );
        assert!(
            mapper
                .map(
                    PipelineEvent::JobRetryScheduled {
                        job_id: "retrying-job".into(),
                    },
                    source
                )
                .is_none()
        );
        let after_retry_scheduled = mapper
            .map(
                PipelineEvent::JobStarted {
                    job_id: "retrying-job".into(),
                    job_type: "rclone".into(),
                    streamer_id: String::new(),
                },
                source,
            )
            .unwrap();
        assert!(
            matches!(after_retry_scheduled, NotificationEvent::PipelineStarted { streamer_id, .. } if streamer_id.is_empty())
        );
    }
}
