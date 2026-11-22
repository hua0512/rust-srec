//! Adaptive concurrency controller and runtime EWMA tracking for worker pools.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use super::{AdaptiveWorkerPoolConfig, WorkerConcurrency, WorkerType};
use crate::pipeline::job_queue::JobQueue;

pub(super) fn update_avg_runtime_ms(avg_runtime_ms: &AtomicU64, sample_ms: u64) {
    // EWMA with alpha=0.2 in integer space: new = old + (sample-old)/5
    let sample_ms = sample_ms.max(1);
    let old = avg_runtime_ms.load(Ordering::Relaxed);
    let next = if old == 0 {
        sample_ms
    } else {
        let delta = sample_ms as i64 - old as i64;
        (old as i64 + delta / 5).max(1) as u64
    };
    avg_runtime_ms.store(next, Ordering::Relaxed);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_adaptive_autoscaler(
    join_set: &mut JoinSet<()>,
    worker_type: WorkerType,
    adaptive: AdaptiveWorkerPoolConfig,
    max_workers: usize,
    concurrency: Arc<WorkerConcurrency>,
    job_queue: Arc<JobQueue>,
    supported_job_types: Arc<Vec<String>>,
    cancellation_token: CancellationToken,
    active_workers: Arc<AtomicUsize>,
    avg_runtime_ms: Arc<AtomicU64>,
) {
    join_set.spawn(async move {
        let interval = std::time::Duration::from_millis(adaptive.interval_ms.max(100));
        let mut idle_ticks: u32 = 0;

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => break,
                _ = tokio::time::sleep(interval) => {}
            }

            let filter_types = if supported_job_types.is_empty() {
                None
            } else {
                Some(supported_job_types.as_slice())
            };

            let pending = match job_queue.count_pending_jobs(filter_types).await {
                Ok(v) => v,
                Err(e) => {
                    warn!(
                        "Adaptive {} scaler failed to count pending jobs: {}",
                        worker_type, e
                    );
                    continue;
                }
            };

            let active = active_workers.load(Ordering::SeqCst) as u64;
            let backlog = pending.saturating_add(active);

            if pending == 0 && active == 0 {
                idle_ticks = idle_ticks.saturating_add(1);
            } else {
                idle_ticks = 0;
            }

            let avg_ms = avg_runtime_ms
                .load(Ordering::Relaxed)
                .max(adaptive.default_avg_runtime_ms.max(1));
            let target_drain_secs = adaptive.target_drain_secs.max(1) as f64;

            let mut target = if backlog == 0 {
                0usize
            } else {
                (((backlog as f64) * (avg_ms as f64) / 1000.0) / target_drain_secs)
                    .ceil()
                    .max(0.0) as usize
            };
            target = target.clamp(adaptive.min_workers.min(max_workers), max_workers);

            let current = concurrency.limit();
            let mut next = current;

            if target > current {
                let step = adaptive.max_step_up.max(1);
                next = (current + step).min(target);
            } else if target < current && idle_ticks >= adaptive.scale_down_idle_ticks.max(1) {
                let step = adaptive.max_step_down.max(1);
                next = current.saturating_sub(step).max(target);
            }

            if next != current {
                let applied = concurrency.set_limit(next);
                debug!(
                    "Adaptive {} pool concurrency: pending={}, active={}, avg_ms={}, desired={}/{} (target={})",
                    worker_type, pending, active, avg_ms, applied, max_workers, target
                );
            }
        }
    });
}
