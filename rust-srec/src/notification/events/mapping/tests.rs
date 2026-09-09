use super::*;
use crate::domain::{StreamerState, streamer::FatalErrorType};
use crate::session::TerminalCause;

fn now() -> DateTime<Utc> {
    DateTime::from_timestamp_millis(1_788_784_496_123).unwrap()
}

#[test]
fn monitor_mapping_only_emits_fatal_alerts_and_keeps_source_time() {
    let source = now();
    for event in [
        MonitorEvent::StreamerLive {
            streamer_id: "s".into(),
            session_id: "session".into(),
            streamer_name: "Name".into(),
            streamer_url: "url".into(),
            title: "title".into(),
            category: None,
            streams: Vec::new(),
            media_headers: None,
            media_extras: None,
            timestamp: source,
        },
        MonitorEvent::StreamerOffline {
            streamer_id: "s".into(),
            streamer_name: "Name".into(),
            session_id: Some("session".into()),
            timestamp: source,
        },
        MonitorEvent::StateChanged {
            streamer_id: "s".into(),
            streamer_name: "Name".into(),
            old_state: StreamerState::Live,
            new_state: StreamerState::OutOfSchedule,
            reason: Some("out_of_schedule".into()),
            timestamp: source,
        },
    ] {
        assert!(monitor_notification(event).is_none());
    }
    let event = monitor_notification(MonitorEvent::FatalError {
        streamer_id: "s".into(),
        streamer_name: "Name".into(),
        error_type: FatalErrorType::NotFound,
        message: "missing".into(),
        new_state: StreamerState::NotFound,
        timestamp: source,
    })
    .unwrap();
    assert!(
        matches!(event, NotificationEvent::FatalError { timestamp, error_type, message, .. } if timestamp == source && error_type == "NotFound" && message == "missing")
    );
}

#[test]
fn session_mapping_suppresses_intermediate_and_operator_endings() {
    let source = now();
    for cause in [
        TerminalCause::UserDisabled,
        TerminalCause::OutOfSchedule,
        TerminalCause::Completed,
    ] {
        let expected_offline = cause == TerminalCause::Completed;
        let event = session_notification(SessionTransition::Ended {
            session_id: "session".into(),
            streamer_id: "s".into(),
            streamer_name: "Name".into(),
            ended_at: source,
            cause,
            via_hysteresis: true,
        });
        if expected_offline {
            assert!(
                matches!(event, Some(NotificationEvent::StreamOffline { timestamp, duration_secs: None, .. }) if timestamp == source)
            );
        } else {
            assert!(event.is_none());
        }
    }
    assert!(
        session_notification(SessionTransition::Ending {
            session_id: "session".into(),
            streamer_id: "s".into(),
            streamer_name: "Name".into(),
            cause: TerminalCause::Completed,
            observed_at: source,
            resume_deadline: source + chrono::Duration::seconds(10)
        })
        .is_none()
    );
    assert!(
        session_notification(SessionTransition::Resumed {
            session_id: "session".into(),
            streamer_id: "s".into(),
            resumed_at: source,
            hysteresis_duration: chrono::Duration::seconds(2)
        })
        .is_none()
    );
    for resumed in [false, true] {
        let event = session_notification(SessionTransition::Started {
            session_id: "session".into(),
            streamer_id: "s".into(),
            streamer_name: "Name".into(),
            title: "Title".into(),
            category: Some("Category".into()),
            started_at: source,
            from_hysteresis: resumed,
            download_start: None,
        })
        .unwrap();
        assert!(
            matches!(event, NotificationEvent::StreamOnline { title, category: Some(category), timestamp, .. } if title == "Title" && category == "Category" && timestamp == source)
        );
    }
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
        matches!(mapper.map(PipelineEvent::QueueWarning { depth: 123 }, source), Some(NotificationEvent::PipelineQueueWarning { queue_depth: 123, threshold: 100, timestamp }) if timestamp == source)
    );
    assert!(
        matches!(mapper.map(PipelineEvent::QueueCritical { depth: 234 }, source), Some(NotificationEvent::PipelineQueueCritical { queue_depth: 234, threshold: 200, timestamp }) if timestamp == source)
    );
}
