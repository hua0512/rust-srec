//! Atomic transactional bundles for session lifecycle operations.
//!
//! [`SessionLifecycleRepository`] is the only place that performs the
//! multi-step DB transactions that combine session-row writes
//! ([`crate::database::repositories::SessionTxOps`]), streamer-state writes
//! ([`crate::database::repositories::StreamerTxOps`]), and monitor-event enqueue
//! ([`crate::database::repositories::MonitorOutboxTxOps`]).

mod bindings;
mod end;
mod start;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use futures::future::BoxFuture;
use jiff::Timestamp;
use sqlx::SqliteConnection;

pub use self::end::{EndForOutOfScheduleInputs, EndSessionInputs, EndSessionOutcome};
pub use self::start::{StartSessionInputs, StartSessionOutcome};
use crate::Result;
use crate::database::committed_writer::commit_owned;
use crate::database::models::SessionEventDbModel;
use crate::database::{WritePool, begin_immediate};
use crate::domain::StreamerState;
use crate::session::events::SessionEventPayload;
use crate::streamer::CommittedStreamerState;
use crate::streamer::state_store::{StateChange, StatePublication};

/// Atomic transactional bundles for session lifecycle.
pub struct SessionLifecycleRepository {
    write_pool: WritePool,
    committed_state: std::sync::OnceLock<Arc<CommittedStreamerState>>,
}

impl SessionLifecycleRepository {
    pub fn new(write_pool: WritePool) -> Self {
        Self {
            write_pool,
            committed_state: std::sync::OnceLock::new(),
        }
    }

    pub(crate) fn bind_committed_state(&self, state: Arc<CommittedStreamerState>) {
        self.committed_state.get_or_init(|| state);
    }

    pub(crate) fn with_committed_state(self, state: Arc<CommittedStreamerState>) -> Self {
        self.bind_committed_state(state);
        self
    }

    /// Run a streamer-state write through the committed store so the changed
    /// row is published to the cache. Without a bound store, as in
    /// repository-only tests, the same operation runs in a plain
    /// `BEGIN IMMEDIATE`.
    async fn state_transaction<T, F>(&self, label: &'static str, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: for<'c> FnOnce(&'c mut SqliteConnection) -> BoxFuture<'c, Result<StateChange<T>>>
            + Send
            + 'static,
    {
        if let Some(store) = self.committed_state.get() {
            store.writer.require_same_pool(&self.write_pool)?;
            return store
                .transaction(label, StatePublication::StateOnly, operation)
                .await;
        }
        let mut tx = begin_immediate(&self.write_pool).await?;
        let change = operation(&mut tx).await?;
        commit_owned(tx).await?;
        Ok(change.value)
    }

    /// Run a write that changes no streamer row through the committed writer,
    /// falling back to a plain `BEGIN IMMEDIATE` like [`Self::state_transaction`].
    async fn write_transaction<T, F>(&self, label: &'static str, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: for<'c> FnOnce(&'c mut SqliteConnection) -> BoxFuture<'c, Result<T>> + Send + 'static,
    {
        if let Some(store) = self.committed_state.get() {
            store.writer.require_same_pool(&self.write_pool)?;
            return store.writer.transaction(label, operation, |_| {}).await;
        }
        let mut tx = begin_immediate(&self.write_pool).await?;
        let value = operation(&mut tx).await?;
        commit_owned(tx).await?;
        Ok(value)
    }

    /// Build the canonical [`SessionEventDbModel`] for a typed payload.
    fn event_row(
        session_id: &str,
        streamer_id: &str,
        payload: &SessionEventPayload,
        occurred_at: Timestamp,
    ) -> Result<SessionEventDbModel> {
        let encoded_payload = serde_json::to_string(payload)?;
        Ok(SessionEventDbModel {
            id: 0,
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            kind: payload.kind().as_str().to_string(),
            occurred_at: occurred_at.as_millisecond(),
            payload: Some(encoded_payload),
        })
    }

    /// Read the streamer row's `state` inside `tx`, returning it only when
    /// it forbids starting or resuming a session.
    async fn inactive_state(
        tx: &mut SqliteConnection,
        streamer_id: &str,
    ) -> Result<Option<StreamerState>> {
        Ok(Self::authoritative_state(tx, streamer_id)
            .await?
            .filter(|state| !state.is_active()))
    }

    async fn authoritative_state(
        tx: &mut SqliteConnection,
        streamer_id: &str,
    ) -> Result<Option<StreamerState>> {
        let row: Option<(String, Option<i64>)> =
            sqlx::query_as("SELECT state, deleted_at FROM streamers WHERE id = ?")
                .bind(streamer_id)
                .fetch_optional(tx)
                .await?;
        Ok(row.and_then(|(state, deleted)| {
            if deleted.is_some() {
                Some(StreamerState::Disabled)
            } else {
                StreamerState::parse(&state)
            }
        }))
    }
}
