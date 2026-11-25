//! Own a database mutation through commit and synchronous publication. The caller
//! may cancel before commit; an in-progress commit must finish under supervision.

use std::sync::Arc;

use futures::future::BoxFuture;
use sqlx::{Connection, SqliteConnection, SqlitePool};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::{DbError as Error, DbResult as Result};
use srec_core::utils::task_supervisor::TaskSupervisor;

struct OwnedOperation {
    cancellation: CancellationToken,
    sealed: std::sync::atomic::AtomicBool,
}

tokio::task_local! {
    static OWNED_OPERATION: Arc<OwnedOperation>;
}

/// Called without an intervening await before COMMIT or irreversible memory
/// publication. Once sealed, the owner finishes all remaining effects.
pub fn prepare_owned_commit() -> Result<()> {
    OWNED_OPERATION
        .try_with(|operation| {
            if !operation.sealed.load(std::sync::atomic::Ordering::Acquire) {
                if operation.cancellation.is_cancelled() {
                    return Err(Error::Other(
                        "Lifecycle mutation cancelled before commit".into(),
                    ));
                }
                operation
                    .sealed
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            Ok(())
        })
        .unwrap_or(Ok(()))
}

/// Commit `tx` immediately after [`prepare_owned_commit`], with no await
/// between the two.
pub async fn commit_owned(tx: sqlx::Transaction<'_, sqlx::Sqlite>) -> Result<()> {
    prepare_owned_commit()?;
    tx.commit().await?;
    Ok(())
}

/// The future must own a bounded admission permit (lifecycle stripe/admission
/// guards). Cancellation still drops the future before its commit boundary.
pub async fn own_operation<T, E, F>(
    supervisor: Arc<TaskSupervisor>,
    future: F,
) -> std::result::Result<T, E>
where
    T: Send + 'static,
    E: From<Error> + std::fmt::Display + Send + 'static,
    F: std::future::Future<Output = std::result::Result<T, E>> + Send + 'static,
{
    let cancellation = CancellationToken::new();
    let cancel_on_drop = cancellation.clone().drop_guard();
    let operation = Arc::new(OwnedOperation {
        cancellation: cancellation.clone(),
        sealed: std::sync::atomic::AtomicBool::new(false),
    });
    let (result_tx, result_rx) = oneshot::channel();
    let owner = supervisor.clone();
    if !supervisor.spawn("owned lifecycle operation", async move {
        let _owner = owner;
        let result = OWNED_OPERATION.scope(operation.clone(), async move {
            let mut future = Box::pin(future);
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => {
                    if operation.sealed.load(std::sync::atomic::Ordering::Acquire) { future.await }
                    else { Err(E::from(Error::Other("Lifecycle mutation cancelled before commit".into()))) }
                }
                result = &mut future => result,
            }
        }).await;
        if let Err(Err(error)) = result_tx.send(result) {
            tracing::debug!(%error, "Lifecycle caller dropped before receiving completion");
        }
    }) {
        return Err(E::from(Error::Other(
            "Lifecycle operation rejected during shutdown".into(),
        )));
    }
    let result = result_rx.await.map_err(|error| {
        E::from(Error::Other(format!(
            "Owned lifecycle operation stopped: {error}"
        )))
    })?;
    cancel_on_drop.disarm();
    result
}

pub struct CommittedWriter {
    pool: SqlitePool,
    supervisor: Arc<TaskSupervisor>,
    #[cfg(any(test, feature = "test-support"))]
    commit_gate: parking_lot::RwLock<Option<(CommitPhase, Arc<CommitTestGate>)>>,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CommitPhase {
    BeforeCommit,
    AfterCommit,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub struct CommitTestGate {
    pub started: tokio::sync::Notify,
    pub release: tokio::sync::Notify,
}

impl CommittedWriter {
    pub fn new(pool: SqlitePool, supervisor: Arc<TaskSupervisor>) -> Result<Self> {
        if pool.options().get_max_connections() != 1 {
            return Err(Error::Configuration(
                "Committed publication requires the shared single-connection writer pool"
                    .to_owned(),
            ));
        }
        Ok(Self::for_invalidation(pool, supervisor))
    }

    /// Invalidation commutes, so cache-invalidating writers may use a larger
    /// pool. Complete-row publication must additionally require_serialized().
    pub fn for_invalidation(pool: SqlitePool, supervisor: Arc<TaskSupervisor>) -> Self {
        Self {
            pool,
            supervisor,
            #[cfg(any(test, feature = "test-support"))]
            commit_gate: parking_lot::RwLock::new(None),
        }
    }

    pub fn require_serialized(&self) -> Result<()> {
        if self.pool.options().get_max_connections() != 1 {
            return Err(Error::Configuration(
                "Complete streamer-row publication requires one shared writer connection"
                    .to_owned(),
            ));
        }
        Ok(())
    }

    pub fn require_same_pool(&self, pool: &SqlitePool) -> Result<()> {
        if !std::ptr::eq(self.pool.options(), pool.options()) {
            return Err(Error::Configuration(
                "Committed streamer writers must share the same writer pool".to_owned(),
            ));
        }
        Ok(())
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_commit_gate(&self, phase: CommitPhase, gate: Option<Arc<CommitTestGate>>) {
        *self.commit_gate.write() = gate.map(|gate| (phase, gate));
    }

    pub async fn transaction<T, F, P>(
        &self,
        label: &'static str,
        operation: F,
        publish: P,
    ) -> Result<T>
    where
        T: Send + 'static,
        F: for<'c> FnOnce(&'c mut SqliteConnection) -> BoxFuture<'c, Result<T>> + Send + 'static,
        P: FnOnce(&T) + Send + 'static,
    {
        self.transaction_with_error(label, operation, publish).await
    }

    pub async fn transaction_with_error<T, E, F, P>(
        &self,
        label: &'static str,
        operation: F,
        publish: P,
    ) -> std::result::Result<T, E>
    where
        T: Send + 'static,
        E: From<sqlx::Error> + From<Error> + std::fmt::Display + Send + 'static,
        F: for<'c> FnOnce(&'c mut SqliteConnection) -> BoxFuture<'c, std::result::Result<T, E>>
            + Send
            + 'static,
        P: FnOnce(&T) + Send + 'static,
    {
        self.transaction_with_completion(label, operation, publish, |value| {
            Box::pin(async move { Ok(value) })
        })
        .await
    }

    /// Completion runs under the same task owner after releasing the writer.
    /// Callers with awaited runtime effects must hold their own bounded admission
    /// permit through completion so releasing the database lease cannot grow work.
    pub async fn transaction_with_completion<T, U, E, F, P, C>(
        &self,
        label: &'static str,
        operation: F,
        publish: P,
        completion: C,
    ) -> std::result::Result<U, E>
    where
        T: Send + 'static,
        U: Send + 'static,
        E: From<sqlx::Error> + From<Error> + std::fmt::Display + Send + 'static,
        F: for<'c> FnOnce(&'c mut SqliteConnection) -> BoxFuture<'c, std::result::Result<T, E>>
            + Send
            + 'static,
        P: FnOnce(&T) + Send + 'static,
        C: FnOnce(T) -> BoxFuture<'static, std::result::Result<U, E>> + Send + 'static,
    {
        if OWNED_OPERATION.try_with(|_| ()).is_ok() {
            let mut connection = self.pool.acquire().await?;
            let mut transaction = connection.begin_with("BEGIN IMMEDIATE").await?;
            let value = operation(&mut transaction).await?;
            prepare_owned_commit().map_err(E::from)?;
            #[cfg(any(test, feature = "test-support"))]
            let commit_gate = self.commit_gate.read().clone();
            #[cfg(any(test, feature = "test-support"))]
            if let Some((CommitPhase::BeforeCommit, gate)) = &commit_gate {
                gate.started.notify_one();
                gate.release.notified().await;
            }
            transaction.commit().await?;
            #[cfg(any(test, feature = "test-support"))]
            if let Some((CommitPhase::AfterCommit, gate)) = &commit_gate {
                gate.started.notify_one();
                gate.release.notified().await;
            }
            publish(&value);
            drop(connection);
            return completion(value).await;
        }
        // Acquiring before spawning bounds owned work to the pool's connection capacity.
        // Waiting callers remain cancellable and own no background task.
        let mut connection = self.pool.acquire().await?;
        let cancellation = CancellationToken::new();
        let cancel_on_drop = cancellation.clone().drop_guard();
        let (result_tx, result_rx) = oneshot::channel();
        let owner = self.supervisor.clone();
        #[cfg(any(test, feature = "test-support"))]
        let commit_gate = self.commit_gate.read().clone();
        if !self.supervisor.spawn(label, async move {
            let _owner = owner;
            let result = async {
                let mut transaction = connection.begin_with("BEGIN IMMEDIATE").await?;
                let value = tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => return Err(E::from(Error::Other("Database mutation cancelled before commit".to_owned()))),
                    value = operation(&mut transaction) => value?,
                };
                if cancellation.is_cancelled() {
                    return Err(E::from(Error::Other("Database mutation cancelled before commit".to_owned())));
                }
                // From this point the owned task, not the request future, settles
                // COMMIT. Retain the outer lease until synchronous publication is
                // complete; no newer participating writer can publish first.
                #[cfg(any(test, feature = "test-support"))]
                if let Some((CommitPhase::BeforeCommit, gate)) = &commit_gate {
                    gate.started.notify_one(); gate.release.notified().await;
                }
                transaction.commit().await?;
                #[cfg(any(test, feature = "test-support"))]
                if let Some((CommitPhase::AfterCommit, gate)) = &commit_gate {
                    gate.started.notify_one(); gate.release.notified().await;
                }
                publish(&value);
                drop(connection);
                completion(value).await
            }.await;
            if let Err(result) = result_tx.send(result)
                && let Err(error) = result
            {
                tracing::debug!(%error, operation = label, "Cancelled database caller no longer receives mutation result");
            }
        }) {
            return Err(E::from(Error::Other("Database mutation rejected during shutdown".to_owned())));
        }
        let result = result_rx.await.map_err(|error| {
            E::from(Error::Other(format!(
                "Owned database mutation stopped without a result: {error}"
            )))
        })?;
        cancel_on_drop.disarm();
        result
    }
}
