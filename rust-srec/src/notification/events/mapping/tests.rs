use super::*;

fn now() -> DateTime<Utc> {
    DateTime::from_timestamp_millis(1_788_784_496_123).unwrap()
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
