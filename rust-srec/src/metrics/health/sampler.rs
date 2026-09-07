use super::DiskSnapshot;
use super::SystemMetricsSnapshot;
use std::sync::Arc;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::time::Duration;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
use tokio::sync::oneshot;

/// Long-lived inventory owned only by the dedicated sampler thread.
struct SysinfoCache {
    /// CPU + memory inventory. Refresh with `refresh_cpu_mem`.
    system: System,
    /// Mounted filesystems. Refresh with `refresh_disks`.
    disks: sysinfo::Disks,
}

impl SysinfoCache {
    fn new() -> Self {
        Self {
            system: System::new_with_specifics(
                RefreshKind::nothing()
                    .with_cpu(CpuRefreshKind::everything())
                    .with_memory(MemoryRefreshKind::everything()),
            ),
            disks: sysinfo::Disks::new(),
        }
    }

    /// In-place refresh of CPU and memory metrics.
    fn refresh_cpu_mem(&mut self) {
        self.system.refresh_cpu_all();
        self.system.refresh_memory();
    }

    /// In-place refresh of the mounted-filesystem inventory.
    fn refresh_disks(&mut self) {
        self.disks.refresh(true);
    }

    /// Global CPU usage percent (0–100).
    fn cpu_usage(&self) -> f32 {
        self.system.global_cpu_usage()
    }

    /// Memory usage percent (0–100). Returns 0 when total is unknown.
    fn memory_usage_pct(&self) -> f32 {
        let total = self.system.total_memory();
        if total == 0 {
            0.0
        } else {
            (self.system.used_memory() as f64 / total as f64 * 100.0) as f32
        }
    }

    /// Copy the current `sysinfo` view into an owned probe snapshot.
    fn snapshot(&self, include_disks: bool) -> SystemMetricsSnapshot {
        let disks = if include_disks {
            self.disks
                .iter()
                .map(|d| DiskSnapshot {
                    mount_point: d.mount_point().to_path_buf(),
                    available_space: d.available_space(),
                    total_space: d.total_space(),
                })
                .collect::<Vec<_>>()
                .into_boxed_slice()
        } else {
            Vec::<DiskSnapshot>::new().into_boxed_slice()
        };

        SystemMetricsSnapshot {
            cpu_usage: self.cpu_usage(),
            memory_usage: self.memory_usage_pct(),
            disks: Arc::from(disks),
        }
    }
}

struct SampleRequest {
    include_disks: bool,
    reply: oneshot::Sender<SystemMetricsSnapshot>,
}

struct PendingSample {
    include_disks: bool,
    reply: oneshot::Receiver<SystemMetricsSnapshot>,
}

/// At most one probe is in flight, even after an async timeout. A blocked OS call
/// stays on this dedicated thread: Tokio runtime shutdown never has to join it.
/// Dropping the sender stops the thread after an in-flight call returns.
pub(super) struct SystemSampler {
    requests: Option<SyncSender<SampleRequest>>,
    pending: Option<PendingSample>,
    latest: SystemMetricsSnapshot,
    failure: Option<String>,
}

impl SystemSampler {
    pub(super) fn new() -> Self {
        let mut cache = None;
        Self::with_sampling_fn(move |include_disks| {
            // Both initialization and refresh may enter blocking filesystem/OS calls.
            let cache = cache.get_or_insert_with(SysinfoCache::new);
            cache.refresh_cpu_mem();
            if include_disks {
                cache.refresh_disks();
            }
            cache.snapshot(include_disks)
        })
    }

    pub(super) fn with_sampling_fn(
        mut sample: impl FnMut(bool) -> SystemMetricsSnapshot + Send + 'static,
    ) -> Self {
        let (requests, receiver) = sync_channel::<SampleRequest>(1);
        let worker = std::thread::Builder::new()
            .name("health-system-sampler".to_owned())
            .spawn(move || {
                while let Ok(request) = receiver.recv() {
                    let snapshot = sample(request.include_disks);
                    // A cancelled checker may no longer be interested in the completed IO.
                    if request.reply.send(snapshot).is_err() {
                        break;
                    }
                }
            });
        let failure = match worker {
            Ok(handle) => {
                // std threads are intentionally detached; a stuck mount cannot delay
                // Tokio's shutdown by keeping a spawn_blocking task alive.
                drop(handle);
                None
            }
            Err(error) => Some(format!("Could not start system sampler: {error}")),
        };
        Self {
            requests: failure.is_none().then_some(requests),
            pending: None,
            latest: SystemMetricsSnapshot::empty(),
            failure,
        }
    }

    pub(super) fn latest(&self) -> SystemMetricsSnapshot {
        self.latest.clone()
    }

    pub(super) async fn sample(
        &mut self,
        include_disks: bool,
        budget: Duration,
    ) -> Result<SystemMetricsSnapshot, String> {
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            if self.pending.is_none() {
                let (reply, receiver) = oneshot::channel();
                let request = SampleRequest {
                    include_disks,
                    reply,
                };
                let sent = self
                    .requests
                    .as_ref()
                    .is_some_and(|sender| sender.try_send(request).is_ok());
                if !sent {
                    let failure = "System sampler worker is unavailable".to_owned();
                    self.failure = Some(failure.clone());
                    return Err(failure);
                }
                self.pending = Some(PendingSample {
                    include_disks,
                    reply: receiver,
                });
            }
            let Some(pending) = self.pending.as_mut() else {
                return Err("System sampler has no pending request".to_owned());
            };
            let sampled_disks = pending.include_disks;
            let result = tokio::time::timeout_at(deadline, &mut pending.reply).await;
            match result {
                Err(_) => {
                    return Err(
                        "System metrics sampling timed out; previous values retained".to_owned(),
                    );
                }
                Ok(Err(_)) => {
                    self.pending = None;
                    let failure =
                        "System sampler worker stopped before returning a sample".to_owned();
                    self.failure = Some(failure.clone());
                    return Err(failure);
                }
                Ok(Ok(snapshot)) => {
                    self.pending = None;
                    self.latest = snapshot;
                    if !include_disks || sampled_disks {
                        return Ok(self.latest());
                    }
                    // A late CPU-only sample cannot satisfy a newly due disk probe.
                    // Request its disk inventory serially within the same overall budget.
                }
            }
        }
    }
}
