use super::*;
use std::sync::atomic::AtomicUsize;

fn sampled(disks: bool) -> SystemMetricsSnapshot {
    SystemMetricsSnapshot {
        cpu_usage: 12.0,
        memory_usage: 34.0,
        disks: if disks {
            Arc::from(vec![DiskSnapshot {
                mount_point: PathBuf::from("/"),
                available_space: 75,
                total_space: 100,
            }])
        } else {
            Arc::from([])
        },
    }
}

#[tokio::test(flavor = "current_thread")]
async fn timed_out_sampling_remains_single_flight_and_late_cpu_requires_disk_refresh() {
    let (release, blocked) = std::sync::mpsc::sync_channel(1);
    let (started, entered) = tokio::sync::oneshot::channel();
    let calls = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let observed = calls.clone();
    let mut started = Some(started);
    let mut sampler = SystemSampler::with_sampling_fn(move |disks| {
        observed.lock().push(disks);
        if let Some(started) = started.take() {
            started.send(()).unwrap();
            blocked.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        sampled(disks)
    });
    let (first, ()) = tokio::join!(sampler.sample(false, Duration::from_millis(30)), async {
        entered.await.unwrap();
        tokio::time::sleep(Duration::from_millis(1)).await;
    },);
    assert!(first.is_err());
    for _ in 0..3 {
        assert!(
            sampler
                .sample(true, Duration::from_millis(10))
                .await
                .is_err()
        );
    }
    assert_eq!(
        *calls.lock(),
        [false],
        "timeouts must not enqueue overlapping probes"
    );
    release.send(()).unwrap();
    let metrics = sampler.sample(true, Duration::from_secs(1)).await.unwrap();
    assert_eq!(*calls.lock(), [false, true]);
    assert_eq!(metrics.disks.len(), 1);
    assert_eq!(metrics.cpu_usage, 12.0);
}

#[tokio::test]
async fn sampler_panic_becomes_a_stable_failure_without_restarting_workers() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let mut sampler = SystemSampler::with_sampling_fn(move |_| {
        observed.fetch_add(1, Ordering::SeqCst);
        std::panic::resume_unwind(Box::new("injected sampler panic"));
    });
    for _ in 0..3 {
        assert!(
            sampler
                .sample(true, Duration::from_secs(1))
                .await
                .unwrap_err()
                .contains("stopped")
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

struct FixtureProbe {
    disk: bool,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl HealthProbe for FixtureProbe {
    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed(if self.disk { "disk:fixture" } else { "cheap" })
    }
    fn cadence(&self) -> Duration {
        Duration::ZERO
    }
    fn needs_disk_snapshot(&self) -> bool {
        self.disk
    }
    async fn probe(&self, metrics: SystemMetricsSnapshot) -> ComponentHealth {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let health = ComponentHealth::healthy(self.name().into_owned());
        if self.disk {
            let disk = &metrics.disks[0];
            HealthChecker::check_disk_space_with_thresholds(
                "fixture",
                disk.available_space,
                disk.total_space,
                0.80,
                0.95,
            )
            .with_disk(DiskUsage::new(
                "fixture",
                "/",
                disk.available_space,
                disk.total_space,
            ))
        } else {
            health
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn blocked_sampling_marks_stale_disks_but_cheap_probes_continue_and_recover() {
    for initially_critical in [false, true] {
        let (release, blocked) = std::sync::mpsc::sync_channel(1);
        let mut attempts = 0;
        let mut sampler = SystemSampler::with_sampling_fn(move |disks| {
            attempts += 1;
            if attempts == 2 {
                blocked.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            let mut metrics = sampled(disks);
            if attempts == 1 && initially_critical {
                Arc::make_mut(&mut metrics.disks)[0].available_space = 1;
            }
            metrics
        });
        let checker = HealthChecker::new();
        let cheap_calls = Arc::new(AtomicUsize::new(0));
        let disk_calls = Arc::new(AtomicUsize::new(0));
        let probes: Vec<Arc<dyn HealthProbe>> = vec![
            Arc::new(FixtureProbe {
                disk: false,
                calls: cheap_calls.clone(),
            }),
            Arc::new(FixtureProbe {
                disk: true,
                calls: disk_calls.clone(),
            }),
        ];
        let mut last_run = HashMap::new();
        checker
            .refresh_due(&probes, &mut last_run, &mut sampler)
            .await;
        assert_eq!(
            checker.current().status,
            if initially_critical {
                HealthStatus::Unhealthy
            } else {
                HealthStatus::Healthy
            }
        );
        checker
            .refresh_due(&probes, &mut last_run, &mut sampler)
            .await;
        let stale = checker.current();
        assert_eq!(stale.cpu_usage, 12.0);
        assert_eq!(stale.memory_usage, 34.0);
        assert_eq!(
            stale.components["system-metrics"].status,
            HealthStatus::Degraded
        );
        assert_eq!(
            stale.components["disk:fixture"].status,
            if initially_critical {
                HealthStatus::Unhealthy
            } else {
                HealthStatus::Degraded
            }
        );
        assert_eq!(
            stale.components["disk:fixture"]
                .disk
                .as_ref()
                .unwrap()
                .available_bytes,
            if initially_critical { 1 } else { 75 }
        );
        assert_eq!(stale.components["cheap"].status, HealthStatus::Healthy);
        assert_eq!(cheap_calls.load(Ordering::SeqCst), 2);
        assert_eq!(disk_calls.load(Ordering::SeqCst), 1);
        release.send(()).unwrap();
        checker
            .refresh_due(&probes, &mut last_run, &mut sampler)
            .await;
        assert_eq!(checker.current().status, HealthStatus::Healthy);
        assert!(!checker.current().components.contains_key("system-metrics"));
        assert_eq!(disk_calls.load(Ordering::SeqCst), 2);
    }
}

#[test]
fn blocked_sampler_does_not_delay_checker_or_tokio_runtime_shutdown() {
    let (release, blocked) = std::sync::mpsc::sync_channel(1);
    let (runtime_done, finished) = std::sync::mpsc::sync_channel(1);
    let (sample_done, returned) = std::sync::mpsc::sync_channel(1);
    let host = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let checker = Arc::new(HealthChecker::new());
            let cancel = CancellationToken::new();
            let (started, entered) = tokio::sync::oneshot::channel();
            let handle = checker.start_with_sampler(cancel.clone(), move || {
                let mut started = Some(started);
                SystemSampler::with_sampling_fn(move |disks| {
                    if let Some(started) = started.take() {
                        started.send(()).unwrap();
                    }
                    blocked.recv_timeout(Duration::from_secs(5)).unwrap();
                    sample_done.send(()).unwrap();
                    sampled(disks)
                })
            });
            tokio::time::timeout(Duration::from_secs(1), entered)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(checker.current().status, HealthStatus::Unknown);
            // A timer on a single-thread runtime still progresses during blocked initial IO.
            tokio::time::sleep(Duration::from_millis(1)).await;
            cancel.cancel();
            tokio::time::timeout(Duration::from_millis(100), handle)
                .await
                .unwrap()
                .unwrap();
        });
        drop(runtime);
        runtime_done.send(()).unwrap();
    });
    let result = finished.recv_timeout(Duration::from_secs(2));
    // Always release the bounded fixture before asserting, including a failing shutdown.
    release.send(()).unwrap();
    host.join().unwrap();
    returned.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(
        result.is_ok(),
        "runtime shutdown waited for the blocked sampler"
    );
}
