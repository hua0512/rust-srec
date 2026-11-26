//! Serialized, unabortable SQLite persistence for the recording control plane.
//!
//! [`RecorderStore`] runs the recorder's database writes on top of
//! `CommittedWriter`, adding:
//! - per-streamer asynchronous write serialization leases so generation `G+1`
//!   always observes the final committed database and cache state of generation
//!   `G` even if `G`'s caller future is dropped mid-commit;
//! - atomic transactional outbox writes (`monitor_event_outbox`) using the
//!   transaction's exact epoch-millisecond timestamp;
//! - post-publication change-set dispatch strictly after both
//!   `StreamerCache::publication` and the SQLite write connection are released.

mod ops;

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use futures::future::BoxFuture;
use parking_lot::Mutex;
use sqlx::SqliteConnection;
use tokio::sync::{Mutex as AsyncMutex, Notify, OwnedMutexGuard};

use crate::config::ConfigEventBroadcaster;
use crate::recorder::ports::{
    DanmuSegmentRecord, DbCommitFailure, DbCommitOp, DbCommitOutcome, VideoSegmentRecord,
};
use crate::streamer::CommittedStreamerState;
use crate::streamer::state_store::{StateChange, StatePublication};
use crate::{Error, Result};
use srec_core::CoreError;
use srec_core::utils::task_supervisor::TaskSupervisor;
use srec_sqlite::committed_writer::{CommittedWriter, own_operation};
use srec_sqlite::models::{
    MediaFileType, MediaOutputDbModel, SessionSegmentDbModel, SessionSegmentLifecycle,
    SessionSegmentSplitReason,
};
use srec_sqlite::repositories::SessionTxOps;
use srec_sqlite::retry::retry_on_sqlite_busy;
use srec_sqlite::time::datetime_to_ms;
use srec_sqlite::{DbPool, WritePool};

/// Change set dispatched after a database transaction commits, `StreamerCache`
/// publishes its snapshot, and both the publication lock and SQLite write
/// connection have been released.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PostPublicationChangeSet {
    /// Streamer IDs whose committed database row was inserted or updated.
    pub updated_streamers: Vec<String>,
    /// Streamer IDs removed from the cache.
    pub removed_streamers: Vec<String>,
    /// Streamer IDs whose configuration or credential selection changed.
    pub reconfigured_streamers: Vec<String>,
    /// Whether one or more notification rows were enqueued into `monitor_event_outbox`.
    pub outbox_enqueued: bool,
}

impl PostPublicationChangeSet {
    /// Returns `true` when no streamer rows changed and no outbox rows were enqueued.
    pub fn is_empty(&self) -> bool {
        self.updated_streamers.is_empty()
            && self.removed_streamers.is_empty()
            && self.reconfigured_streamers.is_empty()
            && !self.outbox_enqueued
    }
}

/// Asynchronous listener notified with a [`PostPublicationChangeSet`] strictly
/// after the SQLite write connection, `StreamerCache::publication` lock, and
/// per-streamer serialization lease have been released.
#[async_trait]
pub trait PostPublicationListener: Send + Sync {
    async fn on_published(&self, changes: PostPublicationChangeSet);
}

struct StreamerCommitLeases {
    locks: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
}

impl Default for StreamerCommitLeases {
    fn default() -> Self {
        Self {
            locks: Mutex::new(HashMap::new()),
        }
    }
}

struct StreamerCommitLease {
    guard: Option<OwnedMutexGuard<()>>,
    key: String,
    locks: Arc<StreamerCommitLeases>,
}

impl Drop for StreamerCommitLease {
    fn drop(&mut self) {
        // Release the owned mutex guard (and its strong Arc reference) BEFORE
        // checking weak.strong_count(), otherwise Drop::drop always observes >= 1.
        drop(self.guard.take());
        let mut map = self.locks.locks.lock();
        if let Some(weak) = map.get(&self.key)
            && weak.strong_count() == 0
        {
            map.remove(&self.key);
        }
    }
}

impl StreamerCommitLeases {
    async fn acquire(self: &Arc<Self>, key: &str) -> StreamerCommitLease {
        let mutex = {
            let mut map = self.locks.lock();
            if let Some(existing) = map.get(key).and_then(Weak::upgrade) {
                existing
            } else {
                let created = Arc::new(AsyncMutex::new(()));
                map.insert(key.to_owned(), Arc::downgrade(&created));
                created
            }
        };
        let guard = mutex.lock_owned().await;
        StreamerCommitLease {
            guard: Some(guard),
            key: key.to_owned(),
            locks: self.clone(),
        }
    }

    fn has_in_flight(&self, key: &str) -> bool {
        self.locks.lock().get(key).and_then(Weak::upgrade).is_some()
    }
}

pub(super) struct TxCommitPayload<T> {
    pub(super) value: T,
    pub(super) state_change: StateChange<()>,
    pub(super) outbox_enqueued: bool,
}

impl<T> TxCommitPayload<T> {
    pub(super) fn plain(value: T) -> Self {
        Self {
            value,
            state_change: StateChange::row((), None),
            outbox_enqueued: false,
        }
    }
}

/// SQLite persistence for recorder actors: serialized per-streamer commits
/// and segment rows.
#[derive(Clone)]
pub struct RecorderStore {
    read_pool: DbPool,
    write_pool: WritePool,
    writer: Arc<CommittedWriter>,
    committed_state: Option<Arc<CommittedStreamerState>>,
    supervisor: Arc<TaskSupervisor>,
    leases: Arc<StreamerCommitLeases>,
    outbox_notify: Option<Arc<Notify>>,
    post_publication: Arc<parking_lot::RwLock<Option<Arc<dyn PostPublicationListener>>>>,
}

impl RecorderStore {
    /// Construct a standalone [`RecorderStore`] backed by `read_pool` and a
    /// single-connection `write_pool`.
    pub fn new(read_pool: DbPool, write_pool: WritePool) -> Result<Self> {
        let supervisor = Arc::new(TaskSupervisor::for_committed_work());
        let writer = Arc::new(CommittedWriter::new(
            write_pool.clone(),
            supervisor.clone(),
        )?);
        Ok(Self {
            read_pool,
            write_pool,
            writer,
            committed_state: None,
            supervisor,
            leases: Arc::new(StreamerCommitLeases::default()),
            outbox_notify: None,
            post_publication: Arc::new(parking_lot::RwLock::new(None)),
        })
    }

    /// Construct a [`RecorderStore`] paired with a fresh [`CommittedStreamerState`]
    /// sharing the same single-connection writer pool.
    pub fn with_new_committed_state(
        read_pool: DbPool,
        write_pool: WritePool,
        broadcaster: ConfigEventBroadcaster,
    ) -> Result<(Self, Arc<CommittedStreamerState>)> {
        let supervisor = Arc::new(TaskSupervisor::for_committed_work());
        let writer = Arc::new(CommittedWriter::new(
            write_pool.clone(),
            supervisor.clone(),
        )?);
        let committed_state = Arc::new(CommittedStreamerState::new(writer.clone(), broadcaster));
        let store = Self {
            read_pool,
            write_pool,
            writer,
            committed_state: Some(committed_state.clone()),
            supervisor,
            leases: Arc::new(StreamerCommitLeases::default()),
            outbox_notify: None,
            post_publication: Arc::new(parking_lot::RwLock::new(None)),
        };
        Ok((store, committed_state))
    }

    /// Construct a [`RecorderStore`] wired to an existing [`CommittedStreamerState`].
    pub fn with_committed_state(
        read_pool: DbPool,
        write_pool: WritePool,
        committed_state: Arc<CommittedStreamerState>,
    ) -> Result<Self> {
        Self::with_committed_state_and_supervisor(
            read_pool,
            write_pool,
            committed_state,
            Arc::new(TaskSupervisor::for_committed_work()),
        )
    }

    /// Construct a [`RecorderStore`] sharing an existing [`CommittedStreamerState`]
    /// and caller-owned committed-write [`TaskSupervisor`].
    pub fn with_committed_state_and_supervisor(
        read_pool: DbPool,
        write_pool: WritePool,
        committed_state: Arc<CommittedStreamerState>,
        supervisor: Arc<TaskSupervisor>,
    ) -> Result<Self> {
        committed_state.writer.require_serialized()?;
        committed_state.writer.require_same_pool(&write_pool)?;
        Ok(Self {
            read_pool,
            write_pool,
            writer: committed_state.writer.clone(),
            committed_state: Some(committed_state),
            supervisor,
            leases: Arc::new(StreamerCommitLeases::default()),
            outbox_notify: None,
            post_publication: Arc::new(parking_lot::RwLock::new(None)),
        })
    }

    /// Attach an outbox wake handle notified strictly after commit and write-connection release.
    pub fn with_outbox_notify(mut self, notify: Arc<Notify>) -> Self {
        self.outbox_notify = Some(notify);
        self
    }

    /// Attach a [`PostPublicationListener`] invoked strictly after commit,
    /// `StreamerCache` publication, write-connection release, and lease release.
    pub fn with_post_publication_listener(
        self,
        listener: Arc<dyn PostPublicationListener>,
    ) -> Self {
        self.set_post_publication_listener(listener);
        self
    }

    /// Late-bind or replace the [`PostPublicationListener`] after construction.
    pub fn set_post_publication_listener(&self, listener: Arc<dyn PostPublicationListener>) {
        *self.post_publication.write() = Some(listener);
    }

    /// Read the active session's committed credential binding, if any.
    pub async fn active_session_credential_binding(
        &self,
        session_id: &str,
    ) -> Result<Option<crate::credentials::CredentialBinding>> {
        let raw: Option<Option<String>> = sqlx::query_scalar(
            "SELECT credential_binding FROM live_sessions WHERE id = ? AND end_time IS NULL",
        )
        .bind(session_id)
        .fetch_optional(&self.read_pool)
        .await?;
        let Some(Some(raw)) = raw else {
            return Ok(None);
        };
        serde_json::from_str(&raw).map(Some).map_err(|error| {
            Error::database(format!("invalid session credential_binding: {error}"))
        })
    }

    /// Read the newest active session and its credential binding for `streamer_id`.
    pub async fn active_streamer_session_binding(
        &self,
        streamer_id: &str,
    ) -> Result<Option<(String, Option<crate::credentials::CredentialBinding>)>> {
        let row: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT id, credential_binding FROM live_sessions \
             WHERE streamer_id = ? AND end_time IS NULL \
             ORDER BY start_time DESC LIMIT 1",
        )
        .bind(streamer_id)
        .fetch_optional(&self.read_pool)
        .await?;
        let Some((session_id, raw)) = row else {
            return Ok(None);
        };
        let binding = raw
            .map(|raw| {
                serde_json::from_str(&raw).map_err(|error| {
                    Error::database(format!("invalid session credential_binding: {error}"))
                })
            })
            .transpose()?;
        Ok(Some((session_id, binding)))
    }

    /// Find an active session bound to a retired credential profile for `streamer_id`.
    pub async fn retiring_credential_session(
        &self,
        streamer_id: &str,
    ) -> Result<Option<(String, crate::credentials::CredentialBinding)>> {
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT s.id, s.credential_binding \
             FROM live_sessions s \
             JOIN retirement_credential_profiles r \
               ON r.profile_id = json_extract(s.credential_binding, '$.identity.profile_id') \
             WHERE s.streamer_id = ? \
               AND s.end_time IS NULL \
               AND s.credential_binding IS NOT NULL \
             ORDER BY s.start_time DESC LIMIT 1",
        )
        .bind(streamer_id)
        .fetch_optional(&self.read_pool)
        .await?;
        let Some((session_id, raw)) = row else {
            return Ok(None);
        };
        let binding = serde_json::from_str(&raw).map_err(|error| {
            Error::database(format!("invalid session credential_binding: {error}"))
        })?;
        Ok(Some((session_id, binding)))
    }

    /// Return the ID of any open (`end_time IS NULL`) session for `streamer_id`.
    pub async fn active_session_id(&self, streamer_id: &str) -> Result<Option<String>> {
        let id: Option<String> = sqlx::query_scalar(
            "SELECT id FROM live_sessions \
             WHERE streamer_id = ? AND end_time IS NULL \
             ORDER BY start_time DESC LIMIT 1",
        )
        .bind(streamer_id)
        .fetch_optional(&self.read_pool)
        .await?;
        Ok(id)
    }

    /// Returns `true` while any database write lease is held or queued for `streamer_id`.
    pub fn has_in_flight_commit(&self, streamer_id: &str) -> bool {
        self.leases.has_in_flight(streamer_id)
    }

    /// Wait until any currently active or queued commit for `streamer_id` has
    /// committed to SQLite and published to `StreamerCache`.
    pub async fn wait_for_streamer_quiescence(&self, streamer_id: &str) {
        let _lease = self.leases.acquire(streamer_id).await;
    }

    /// Access the bound [`CommittedStreamerState`], if any.
    pub fn committed_state(&self) -> Option<&Arc<CommittedStreamerState>> {
        self.committed_state.as_ref()
    }

    /// Access the underlying write pool.
    pub fn write_pool(&self) -> &WritePool {
        &self.write_pool
    }

    async fn resolve_lease_key(&self, op: &DbCommitOp) -> Result<String> {
        match op {
            DbCommitOp::StartOrResumeSession { streamer_id, .. }
            | DbCommitOp::ResumeHysteresis { streamer_id, .. }
            | DbCommitOp::EndSession { streamer_id, .. }
            | DbCommitOp::EndForCredentialRetirement { streamer_id, .. }
            | DbCommitOp::CommitCredentialBinding { streamer_id, .. }
            | DbCommitOp::RecordFilteredState { streamer_id, .. }
            | DbCommitOp::RecordFatalError { streamer_id, .. }
            | DbCommitOp::RecordTransientError { streamer_id, .. }
            | DbCommitOp::ClearErrors { streamer_id, .. }
            | DbCommitOp::ClearLastError { streamer_id }
            | DbCommitOp::RecordHysteresisEntered { streamer_id, .. } => Ok(streamer_id.clone()),
            DbCommitOp::RewriteEndedCause { session_id, .. }
            | DbCommitOp::AddSessionTitle { session_id, .. } => {
                self.resolve_session_lease_key(session_id).await
            }
        }
    }

    async fn resolve_session_lease_key(&self, session_id: &str) -> Result<String> {
        let streamer_id: Option<Option<String>> =
            sqlx::query_scalar("SELECT streamer_id FROM live_sessions WHERE id = ?")
                .bind(session_id)
                .fetch_optional(&self.read_pool)
                .await?;
        Ok(streamer_id
            .flatten()
            .unwrap_or_else(|| format!("session:{session_id}")))
    }

    /// Execute a database transaction under `lease_key`'s per-streamer write
    /// lease and the unabortable [`CommittedWriter`] owner.
    ///
    /// Ordering guarantee:
    /// 1. Acquire `StreamerCommitLease` (caller remains cancellable while queued).
    /// 2. Move the lease into an owned task so sealed commits finish even if
    ///    the caller future is dropped.
    /// 3. Commit in `BEGIN IMMEDIATE`, synchronously publish to `StreamerCache`
    ///    (under `publication.write()`), and drop the SQLite write connection.
    /// 4. Drop the per-streamer serialization lease so generation `G+1` can
    ///    observe the final committed state.
    /// 5. Wake `outbox_notify` and notify `PostPublicationListener` with no
    ///    cache lock, write connection, or streamer lease held.
    async fn run_serialized_tx<T, F>(
        &self,
        lease_key: &str,
        label: &'static str,
        operation: F,
    ) -> Result<T>
    where
        T: Send + 'static,
        F: for<'c> FnOnce(&'c mut SqliteConnection) -> BoxFuture<'c, Result<TxCommitPayload<T>>>
            + Clone
            + Send
            + Sync
            + 'static,
    {
        let lease = self.leases.acquire(lease_key).await;
        let writer = self.writer.clone();
        let committed_state = self.committed_state.clone();
        let outbox_notify = self.outbox_notify.clone();
        let post_publication = self.post_publication.read().clone();

        own_operation(self.supervisor.clone(), async move {
            let (value, change_set) = retry_on_sqlite_busy(label, || {
                let writer = writer.clone();
                let committed_state = committed_state.clone();
                let operation = operation.clone();
                async move {
                    let payload = writer
                        .transaction_with_error(label, operation, move |payload| {
                            if let Some(state) = committed_state.as_ref()
                                && (!payload.state_change.rows.is_empty()
                                    || !payload.state_change.removed.is_empty())
                            {
                                state
                                    .cache
                                    .apply(&payload.state_change, StatePublication::StateOnly);
                            }
                        })
                        .await?;
                    let change_set = PostPublicationChangeSet {
                        updated_streamers: payload
                            .state_change
                            .rows
                            .into_iter()
                            .map(|row| row.id)
                            .collect(),
                        removed_streamers: payload.state_change.removed,
                        reconfigured_streamers: payload.state_change.reconfigured,
                        outbox_enqueued: payload.outbox_enqueued,
                    };
                    Ok::<_, crate::Error>((payload.value, change_set))
                }
            })
            .await?;

            // Release the per-streamer serialization lease only after COMMIT,
            // cache publication, and write-connection release have all settled.
            drop(lease);

            if change_set.outbox_enqueued
                && let Some(notify) = outbox_notify
            {
                notify.notify_one();
            }

            if let Some(listener) = post_publication
                && !change_set.is_empty()
            {
                listener.on_published(change_set).await;
            }

            Ok(value)
        })
        .await
    }
}

impl RecorderStore {
    /// Run one [`DbCommitOp`] under the streamer's write lease.
    pub async fn commit(
        &self,
        op: DbCommitOp,
    ) -> std::result::Result<DbCommitOutcome, DbCommitFailure> {
        let lease_key =
            self.resolve_lease_key(&op)
                .await
                .map_err(|error| DbCommitFailure::Transient {
                    message: error.to_string(),
                })?;
        let label = ops::commit_op_label(&op);
        let result = self
            .run_serialized_tx(&lease_key, label, move |tx| {
                Box::pin(ops::execute_commit_op(tx, op))
            })
            .await;

        result.map_err(map_commit_error)
    }

    /// Insert a finished video segment and its media output row, once: the
    /// registry persists a segment again when an actor that panicked may
    /// already have committed it.
    pub async fn persist_video_segment(&self, record: VideoSegmentRecord) -> Result<()> {
        let lease_key = record.streamer_id.clone();
        self.run_serialized_tx(&lease_key, "recorder persist_video_segment", move |tx| {
            Box::pin(async move {
                let persisted: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM session_segments \
                     WHERE session_id = ? AND file_path = ?)",
                )
                .bind(&record.session_id)
                .bind(&record.segment_path)
                .fetch_one(&mut *tx)
                .await?;
                if persisted {
                    return Ok(TxCommitPayload::plain(()));
                }
                let size_bytes = i64::try_from(record.size_bytes).unwrap_or(i64::MAX);
                let output = MediaOutputDbModel::new(
                    &record.session_id,
                    &record.segment_path,
                    MediaFileType::Video,
                    size_bytes,
                );
                let segment = SessionSegmentDbModel::new(
                    &record.session_id,
                    record.segment_index,
                    &record.segment_path,
                    record.duration_secs,
                    record.size_bytes,
                    SessionSegmentLifecycle::new(
                        record.started_at.map(datetime_to_ms),
                        Some(datetime_to_ms(record.completed_at)),
                    ),
                    SessionSegmentSplitReason::new(
                        record.split_reason_code,
                        record.split_reason_details_json,
                    ),
                );
                SessionTxOps::insert_media_output(&mut *tx, &output).await?;
                SessionTxOps::insert_segment(&mut *tx, &segment).await?;
                Ok(TxCommitPayload::plain(()))
            })
        })
        .await
    }

    /// Insert a finished danmu XML segment's media output row, once, like
    /// [`Self::persist_video_segment`].
    pub async fn persist_danmu_segment(&self, record: DanmuSegmentRecord) -> Result<()> {
        let lease_key = record.streamer_id.clone();
        self.run_serialized_tx(&lease_key, "recorder persist_danmu_segment", move |tx| {
            Box::pin(async move {
                let persisted: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM media_outputs \
                     WHERE session_id = ? AND file_type = ? AND file_path = ?)",
                )
                .bind(&record.session_id)
                .bind(MediaFileType::DanmuXml.as_str())
                .bind(&record.output_path)
                .fetch_one(&mut *tx)
                .await?;
                if persisted {
                    return Ok(TxCommitPayload::plain(()));
                }
                let size_bytes = i64::try_from(record.size_bytes).unwrap_or(i64::MAX);
                let output = MediaOutputDbModel::new(
                    &record.session_id,
                    &record.output_path,
                    MediaFileType::DanmuXml,
                    size_bytes,
                );
                SessionTxOps::insert_media_output(&mut *tx, &output).await?;
                Ok(TxCommitPayload::plain(()))
            })
        })
        .await
    }

    /// Delete the media output rows of a danmu XML segment whose paired video
    /// segment was discarded.
    pub async fn remove_danmu_segment_output(
        &self,
        session_id: &str,
        danmu_path: &str,
    ) -> Result<()> {
        let lease_key = self.resolve_session_lease_key(session_id).await?;
        let session_id = session_id.to_owned();
        let danmu_path = danmu_path.to_owned();
        self.run_serialized_tx(
            &lease_key,
            "recorder remove_danmu_segment_output",
            move |tx| {
                Box::pin(async move {
                    let output_ids: Vec<String> = sqlx::query_scalar(
                        "SELECT id FROM media_outputs \
                         WHERE session_id = ? AND file_type = ? AND file_path = ?",
                    )
                    .bind(&session_id)
                    .bind(MediaFileType::DanmuXml.as_str())
                    .bind(&danmu_path)
                    .fetch_all(&mut *tx)
                    .await?;
                    for id in output_ids {
                        SessionTxOps::delete_media_output(&mut *tx, &id).await?;
                    }
                    Ok(TxCommitPayload::plain(()))
                })
            },
        )
        .await
    }

    /// Next unused `segment_index` for `session_id`.
    pub async fn next_session_segment_index(&self, session_id: &str) -> Result<u32> {
        let next: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(segment_index), -1) + 1 \
             FROM session_segments WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&self.read_pool)
        .await?;

        u32::try_from(next).map_err(|_| {
            Error::database(format!(
                "next session segment index {next} for session {session_id} is outside u32 range"
            ))
        })
    }
}

fn map_commit_error(error: Error) -> DbCommitFailure {
    match error {
        Error::Core(CoreError::CredentialProfile(_) | CoreError::NotFound { .. }) => {
            DbCommitFailure::CredentialBindingConflict {
                message: error.to_string(),
            }
        }
        other => DbCommitFailure::Transient {
            message: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::session::TerminalCause;
    use jiff::Timestamp;
    use srec_core::domain::StreamerState;
    use srec_sqlite::committed_writer::{CommitPhase, CommitTestGate};
    use srec_sqlite::models::StreamerDbModel;
    use srec_sqlite::repositories::{SqlxStreamerRepository, StreamerRepository as _};

    #[tokio::test]
    async fn dropped_caller_after_commit_seals_still_commits_before_next_generation_runs() {
        tokio::time::timeout(Duration::from_secs(15), async {
            let pool = srec_sqlite::init_pool_with_size("sqlite::memory:", 1)
                .await
                .unwrap();
            srec_sqlite::run_migrations(&pool).await.unwrap();

            let mut streamer = StreamerDbModel::new(
                "Sealed Streamer".to_owned(),
                "https://example.test/s-sealed".to_owned(),
                "platform-bilibili",
            );
            streamer.id = "s-sealed".to_owned();
            SqlxStreamerRepository::new(pool.clone(), pool.clone())
                .create_streamer(&streamer)
                .await
                .unwrap();

            let (store, committed_state) = RecorderStore::with_new_committed_state(
                pool.clone(),
                pool.clone(),
                ConfigEventBroadcaster::new(),
            )
            .unwrap();

            // Pause generation G immediately after `prepare_owned_commit()` seals the commit.
            let gate = Arc::new(CommitTestGate::default());
            store
                .writer
                .set_commit_gate(CommitPhase::BeforeCommit, Some(gate.clone()));

            let t_start = Timestamp::from_millisecond(1_735_050_000_000).unwrap();
            let arrived = gate.started.notified();
            let mut gen_g = Box::pin(store.commit(DbCommitOp::StartOrResumeSession {
                streamer_id: "s-sealed".into(),
                streamer_name: "Sealed Streamer".into(),
                title: "Sealed Start".into(),
                category: None,
                current_avatar: None,
                new_avatar: None,
                credential_binding: None,
                now: t_start,
            }));

            tokio::select! {
                _ = arrived => {}
                res = &mut gen_g => panic!("gen_g must pause at BeforeCommit gate, got {res:?}"),
            }
            assert!(store.has_in_flight_commit("s-sealed"));
            assert!(
                committed_state.get_streamer("s-sealed").is_none(),
                "cache publication must not occur before COMMIT completes"
            );

            // Drop generation G's caller future AFTER commit is sealed.
            drop(gen_g);
            store
                .writer
                .set_commit_gate(CommitPhase::BeforeCommit, None);

            // Launch generation G+1 immediately; it must block on the per-streamer lease
            // until G finishes COMMIT and publishes to StreamerCache.
            let t_end = Timestamp::from_millisecond(1_735_050_060_000).unwrap();
            let gen_g_plus_1 = tokio::spawn({
                let store = store.clone();
                async move {
                    store
                        .commit(DbCommitOp::EndSession {
                            streamer_id: "s-sealed".into(),
                            streamer_name: "Sealed Streamer".into(),
                            session_id: None,
                            cause: TerminalCause::StreamerOffline,
                            via_hysteresis: false,
                            update_streamer_state: Some(StreamerState::NotLive),
                            clear_errors: false,
                            checks_restored_threshold: None,
                            infra_block: None,
                            suppress_offline_notification: false,
                            now: t_end,
                        })
                        .await
                }
            });

            tokio::task::yield_now().await;
            assert!(
                !gen_g_plus_1.is_finished(),
                "G+1 must wait on the per-streamer lease while sealed G finishes"
            );

            gate.release.notify_one();

            let end_outcome = gen_g_plus_1.await.unwrap().unwrap();
            match end_outcome {
                DbCommitOutcome::SessionEnded {
                    session_id: Some(_),
                    transitioned_from_open: true,
                    ..
                } => {}
                other => {
                    panic!("G+1 must observe and close G's sealed session, got {other:?}")
                }
            }
            assert_eq!(
                committed_state.get_streamer("s-sealed").unwrap().state,
                StreamerState::NotLive
            );
            assert!(
                store.leases.locks.lock().is_empty(),
                "dropped leases must remove their entry from the per-streamer lease map"
            );
        })
        .await
        .expect("sealed commit drop handoff test must finish");
    }
}
