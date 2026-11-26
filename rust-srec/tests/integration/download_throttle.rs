//! Cross-crate integration tests between `srec-recorder::downloader::DownloadManager`
//! and `srec-pipeline::ThrottleController`.

use std::sync::Arc;
use std::time::Duration;

use rust_srec::downloader::{DownloadManager, DownloadManagerConfig};
use rust_srec::pipeline::{Job, JobQueue, ThrottleConfig, ThrottleController};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn throttle_monitor_teardown_restores_latest_configured_capacity() {
    for abort in [false, true] {
        let manager = Arc::new(DownloadManager::with_config(DownloadManagerConfig {
            max_concurrent_downloads: 10,
            high_priority_extra_slots: 1,
            ..Default::default()
        }));
        let controller = Arc::new(ThrottleController::new(ThrottleConfig {
            enabled: true,
            critical_threshold: 1,
            warning_threshold: 1,
            check_interval_ms: 10,
            ..Default::default()
        }));
        let queue = Arc::new(JobQueue::with_repository(
            Default::default(),
            srec_pipeline::test_support::job_repository().await,
        ));
        for _ in 0..2 {
            queue
                .enqueue(Job::new("held", vec![], vec![], "", ""))
                .await
                .unwrap();
        }
        let cancel = CancellationToken::new();
        let mut events = controller.subscribe();
        let task = controller
            .clone()
            .start_monitoring(queue, manager.clone(), cancel.clone());
        tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(manager.queue_normal_capacity_for_test(), 5);
        assert_eq!(manager.set_max_concurrent_downloads(8), 8);
        assert_eq!(manager.queue_normal_capacity_for_test(), 4);
        if abort {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            cancel.cancel();
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap();
        }
        assert!(!controller.is_throttled());
        assert_eq!(manager.max_concurrent_downloads(), 8);
        assert_eq!(manager.queue_normal_capacity_for_test(), 8);
        assert_eq!(manager.queue_high_extra_capacity_for_test(), 1);
    }
}
