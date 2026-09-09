use super::*;
use crate::session::{SessionTransition, TerminalCause};

fn ended(cause: TerminalCause) -> SessionTransition {
    SessionTransition::Ended {
        session_id: "session".into(),
        streamer_id: "streamer".into(),
        streamer_name: "Name".into(),
        ended_at: DateTime::from_timestamp_millis(1_788_784_496_123).unwrap(),
        cause,
        via_hysteresis: false,
    }
}

#[tokio::test]
async fn session_listener_applies_mapping_and_closes_without_duplicate_notifications() {
    let service = Arc::new(NotificationService::new());
    let mut notifications = service.subscribe();
    let (tx, rx) = broadcast::channel(16);
    service.listen_for_session_transitions(rx);
    tx.send(ended(TerminalCause::UserDisabled)).unwrap();
    tx.send(ended(TerminalCause::OutOfSchedule)).unwrap();
    tx.send(SessionTransition::Started {
        session_id: "session".into(),
        streamer_id: "streamer".into(),
        streamer_name: "Name".into(),
        title: "Title".into(),
        category: Some("Category".into()),
        started_at: DateTime::from_timestamp_millis(123).unwrap(),
        from_hysteresis: true,
        download_start: None,
    })
    .unwrap();
    let notification = tokio::time::timeout(Duration::from_secs(1), notifications.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(notification, NotificationEvent::StreamOnline { timestamp, category: Some(category), .. } if timestamp.timestamp_millis() == 123 && category == "Category")
    );
    drop(tx);
    assert!(
        tokio::time::timeout(
            Duration::from_secs(1),
            service.task_supervisor.shutdown(Duration::from_secs(1))
        )
        .await
        .unwrap()
    );
    assert!(notifications.try_recv().is_err());
}

#[tokio::test]
async fn all_listener_loops_exit_on_source_close_or_service_cancellation() {
    for cancel in [false, true] {
        let service = Arc::new(NotificationService::new());
        let (monitor_tx, monitor_rx) = broadcast::channel(1);
        let (download_tx, download_rx) = broadcast::channel(1);
        let (pipeline_tx, pipeline_rx) = broadcast::channel(1);
        let (session_tx, session_rx) = broadcast::channel(1);
        service.start_event_listeners(monitor_rx, download_rx, pipeline_rx, session_rx);
        if cancel {
            tokio::time::timeout(Duration::from_secs(1), service.stop())
                .await
                .unwrap();
            assert_eq!(monitor_tx.receiver_count(), 0);
            assert_eq!(download_tx.receiver_count(), 0);
            assert_eq!(pipeline_tx.receiver_count(), 0);
            assert_eq!(session_tx.receiver_count(), 0);
        } else {
            drop((monitor_tx, download_tx, pipeline_tx, session_tx));
            assert!(
                tokio::time::timeout(
                    Duration::from_secs(1),
                    service.task_supervisor.shutdown(Duration::from_secs(1))
                )
                .await
                .unwrap()
            );
        }
    }
}
