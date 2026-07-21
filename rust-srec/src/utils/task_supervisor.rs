//! Ownership and shutdown for application background tasks.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::FutureExt;
use parking_lot::Mutex;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::{Instant, timeout_at};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

/// The first fatal background-task failure observed by the runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RuntimeFailure {
    pub(crate) task: &'static str,
    pub(crate) error: String,
}

impl std::fmt::Display for RuntimeFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "critical task '{}' failed: {}",
            self.task, self.error
        )
    }
}

/// Owns background tasks spawned by the application composition root.
///
/// Once shutdown starts, new tasks are rejected. Existing tasks are joined
/// against one shared deadline and forcibly aborted if they do not finish.
pub(crate) struct TaskSupervisor {
    accepting: AtomicBool,
    tasks: Mutex<JoinSet<&'static str>>,
    cancellation_token: CancellationToken,
    failure_tx: watch::Sender<Option<RuntimeFailure>>,
}

impl Default for TaskSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskSupervisor {
    pub(crate) fn new() -> Self {
        Self::with_cancellation(CancellationToken::new())
    }

    pub(crate) fn with_cancellation(cancellation_token: CancellationToken) -> Self {
        let (failure_tx, _) = watch::channel(None);
        Self {
            accepting: AtomicBool::new(true),
            tasks: Mutex::new(JoinSet::new()),
            cancellation_token,
            failure_tx,
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

    /// Spawns a task whose unexpected exit makes the runtime unhealthy.
    ///
    /// The first returned error, panic, or completion before shutdown is
    /// published to failure subscribers and cancels the runtime token.
    pub(crate) fn spawn_critical<F, E>(&self, name: &'static str, task: F) -> bool
    where
        F: Future<Output = std::result::Result<(), E>> + Send + 'static,
        E: std::fmt::Display + Send + 'static,
    {
        if !self.accepting.load(Ordering::Acquire) {
            warn!(task = name, "Rejecting critical task during shutdown");
            return false;
        }

        let mut tasks = self.tasks.lock();
        if !self.accepting.load(Ordering::Acquire) {
            warn!(task = name, "Rejecting critical task during shutdown");
            return false;
        }

        Self::reap_finished(&mut tasks);
        let cancellation_token = self.cancellation_token.clone();
        let failure_tx = self.failure_tx.clone();
        tasks.spawn(async move {
            let outcome = AssertUnwindSafe(task).catch_unwind().await;
            let failure = match outcome {
                Ok(Ok(())) if cancellation_token.is_cancelled() => None,
                Ok(Ok(())) => Some(RuntimeFailure {
                    task: name,
                    error: "task exited unexpectedly".to_string(),
                }),
                Ok(Err(error)) if cancellation_token.is_cancelled() => {
                    debug!(task = name, error = %error, "Critical task stopped during shutdown");
                    None
                }
                Ok(Err(error)) => Some(RuntimeFailure {
                    task: name,
                    error: error.to_string(),
                }),
                Err(payload) => Some(RuntimeFailure {
                    task: name,
                    error: panic_message(payload),
                }),
            };

            if let Some(failure) = failure {
                let published = failure_tx.send_if_modified(|current| {
                    if current.is_some() {
                        false
                    } else {
                        *current = Some(failure.clone());
                        true
                    }
                });
                if published {
                    error!(task = name, error = %failure.error, "Critical runtime task failed");
                    cancellation_token.cancel();
                }
            }

            name
        });
        true
    }

    pub(crate) async fn wait_for_failure(&self) -> RuntimeFailure {
        let mut receiver = self.failure_tx.subscribe();
        loop {
            if let Some(failure) = receiver.borrow().clone() {
                return failure;
            }
            if receiver.changed().await.is_err() {
                return RuntimeFailure {
                    task: "task supervisor",
                    error: "runtime failure channel closed unexpectedly".to_string(),
                };
            }
        }
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

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "task panicked with a non-string payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::Notify;
    use tokio_util::sync::CancellationToken;

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

    #[tokio::test]
    async fn auxiliary_completion_does_not_signal_runtime_failure() {
        let supervisor = TaskSupervisor::new();
        let mut failure_rx = supervisor.failure_tx.subscribe();
        let (completed_tx, completed_rx) = tokio::sync::oneshot::channel();

        assert!(supervisor.spawn("auxiliary", async move {
            let _ = completed_tx.send(());
        }));
        completed_rx.await.expect("auxiliary task should complete");

        assert!(
            tokio::time::timeout(Duration::from_millis(25), failure_rx.changed())
                .await
                .is_err()
        );
        assert!(supervisor.shutdown(Duration::from_secs(1)).await);
    }

    #[tokio::test]
    async fn critical_error_signals_failure_and_cancels_runtime() {
        let cancellation_token = CancellationToken::new();
        let supervisor = TaskSupervisor::with_cancellation(cancellation_token.clone());

        assert!(
            supervisor.spawn_critical("critical error", async { Err::<(), _>("test failure") })
        );
        let failure = tokio::time::timeout(Duration::from_secs(1), supervisor.wait_for_failure())
            .await
            .expect("critical failure should be published");

        assert_eq!(failure.task, "critical error");
        assert_eq!(failure.error, "test failure");
        assert!(cancellation_token.is_cancelled());
        assert!(supervisor.shutdown(Duration::from_secs(1)).await);
    }

    #[tokio::test]
    async fn critical_panic_signals_failure() {
        let supervisor = TaskSupervisor::new();

        assert!(supervisor.spawn_critical("critical panic", async {
            panic!("test panic");
            #[expect(
                unreachable_code,
                reason = "typed return value follows the intentional panic in this test"
            )]
            Ok::<(), &'static str>(())
        }));
        let failure = tokio::time::timeout(Duration::from_secs(1), supervisor.wait_for_failure())
            .await
            .expect("critical panic should be published");

        assert_eq!(failure.task, "critical panic");
        assert_eq!(failure.error, "test panic");
        assert!(supervisor.shutdown(Duration::from_secs(1)).await);
    }
}
