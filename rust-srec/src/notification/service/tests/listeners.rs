use super::*;

#[tokio::test]
async fn all_listener_loops_exit_on_source_close_or_service_cancellation() {
    for cancel in [false, true] {
        let service = Arc::new(NotificationService::new());
        let (download_tx, download_rx) = broadcast::channel(1);
        let (pipeline_tx, pipeline_rx) = broadcast::channel(1);
        service.start_event_listeners(download_rx, pipeline_rx);
        if cancel {
            tokio::time::timeout(Duration::from_secs(1), service.stop())
                .await
                .unwrap();
            assert_eq!(download_tx.receiver_count(), 0);
            assert_eq!(pipeline_tx.receiver_count(), 0);
        } else {
            drop((download_tx, pipeline_tx));
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
