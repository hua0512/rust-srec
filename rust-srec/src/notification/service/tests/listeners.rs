use super::*;

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
