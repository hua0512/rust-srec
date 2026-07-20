//! Ownership and shutdown for application background tasks.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::task::JoinSet;
use tokio::time::{Instant, timeout_at};
use tracing::{debug, warn};

/// Owns background tasks spawned by the application composition root.
///
/// Once shutdown starts, new tasks are rejected. Existing tasks are joined
/// against one shared deadline and forcibly aborted if they do not finish.
pub(crate) struct TaskSupervisor {
    accepting: AtomicBool,
    tasks: Mutex<JoinSet<&'static str>>,
}

impl Default for TaskSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskSupervisor {
    pub(crate) fn new() -> Self {
        Self {
            accepting: AtomicBool::new(true),
            tasks: Mutex::new(JoinSet::new()),
        }
    }

    /// Spawns `task` under supervisor ownership.
    ///
    /// Returns `false` if shutdown has already started and the task was rejected.
    pub(crate) fn spawn<F>(&self, name: &'static str, task: F) -> bool
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if !self.accepting.load(Ordering::Acquire) {
            warn!(task = name, "Rejecting background task during shutdown");
            return false;
        }

        let mut tasks = self.tasks.lock();
        if !self.accepting.load(Ordering::Acquire) {
            warn!(task = name, "Rejecting background task during shutdown");
            return false;
        }

        Self::reap_finished(&mut tasks);
        tasks.spawn(async move {
            task.await;
            name
        });
        true
    }

    fn reap_finished(tasks: &mut JoinSet<&'static str>) {
        while let Some(result) = tasks.try_join_next() {
            match result {
                Ok(name) => debug!(task = name, "Background task completed"),
                Err(error) => warn!(error = %error, "Background task failed"),
            }
        }
    }

    #[cfg(test)]
    fn task_count(&self) -> usize {
        self.tasks.lock().len()
    }

    /// Stops accepting tasks and waits for every owned task until `timeout` expires.
    ///
    /// Returns `true` when all tasks finished without exceeding the deadline.
    pub(crate) async fn shutdown(&self, timeout: Duration) -> bool {
        self.accepting.store(false, Ordering::Release);

        let mut tasks = {
            let mut owned = self.tasks.lock();
            std::mem::take(&mut *owned)
        };
        let deadline = Instant::now() + timeout;

        while !tasks.is_empty() {
            match timeout_at(deadline, tasks.join_next()).await {
                Ok(Some(Ok(name))) => debug!(task = name, "Background task stopped"),
                Ok(Some(Err(error))) => {
                    warn!(error = %error, "Background task failed while shutting down")
                }
                Ok(None) => break,
                Err(_) => {
                    let unfinished = tasks.len();
                    warn!(
                        unfinished,
                        "Background task shutdown deadline exceeded; aborting tasks"
                    );
                    tasks.abort_all();
                    while let Some(result) = tasks.join_next().await {
                        if let Err(error) = result
                            && !error.is_cancelled()
                        {
                            warn!(error = %error, "Background task failed while being aborted");
                        }
                    }
                    return false;
                }
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::Notify;

    use super::TaskSupervisor;

    #[tokio::test]
    async fn shutdown_waits_for_owned_tasks() {
        let supervisor = TaskSupervisor::new();
        let completed = Arc::new(Notify::new());
        let completed_task = completed.clone();

        assert!(supervisor.spawn("test", async move {
            completed_task.notify_one();
        }));
        completed.notified().await;

        assert!(supervisor.shutdown(std::time::Duration::from_secs(1)).await);
        assert_eq!(supervisor.task_count(), 0);
    }

    #[tokio::test]
    async fn shutdown_aborts_tasks_after_deadline() {
        let supervisor = TaskSupervisor::new();
        assert!(supervisor.spawn("pending", std::future::pending()));

        assert!(!supervisor.shutdown(std::time::Duration::ZERO).await);
        assert_eq!(supervisor.task_count(), 0);
    }

    #[tokio::test]
    async fn shutdown_rejects_late_tasks() {
        let supervisor = TaskSupervisor::new();
        assert!(supervisor.shutdown(std::time::Duration::from_secs(1)).await);
        assert!(!supervisor.spawn("late", async {}));
    }

    #[tokio::test]
    async fn spawn_reaps_completed_tasks() {
        let supervisor = TaskSupervisor::new();
        let (completed_tx, completed_rx) = tokio::sync::oneshot::channel();

        assert!(supervisor.spawn("completed", async move {
            let _ = completed_tx.send(());
        }));
        completed_rx.await.expect("task should complete");

        assert!(supervisor.spawn("pending", std::future::pending()));
        assert_eq!(supervisor.task_count(), 1);

        assert!(!supervisor.shutdown(std::time::Duration::ZERO).await);
    }
}
