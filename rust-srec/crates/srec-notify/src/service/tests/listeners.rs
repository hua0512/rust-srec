use super::*;

#[tokio::test]
async fn all_listener_loops_exit_on_source_close_or_service_cancellation() {
    for cancel in [false, true] {
        let service = Arc::new(NotificationService::new());
        let (first_tx, first_rx) = broadcast::channel::<u32>(1);
        let (second_tx, second_rx) = broadcast::channel::<u32>(1);
        service.spawn_event_listener("test listener 1", first_rx, |_, _| None);
        service.spawn_event_listener("test listener 2", second_rx, |_, _| None);
        if cancel {
            tokio::time::timeout(Duration::from_secs(1), service.stop())
                .await
                .unwrap();
            assert_eq!(first_tx.receiver_count(), 0);
            assert_eq!(second_tx.receiver_count(), 0);
        } else {
            drop((first_tx, second_tx));
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
