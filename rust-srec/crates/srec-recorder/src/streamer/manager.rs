//! Streamer manager implementation.
//!
//! The StreamerManager maintains in-memory streamer metadata with
//! write-through persistence to the database.

use std::sync::Arc;

use dashmap::DashMap;
use tracing::{debug, info, warn};

use crate::Result;
use crate::config::{ConfigEventBroadcaster, ConfigUpdateEvent};
use srec_core::domain::{Priority, StreamerState};
use srec_sqlite::repositories::streamer::StreamerRepository;

use super::metadata::StreamerMetadata;
use super::state_store::{CommittedStreamerState, StateChange, StatePublication, StreamerCache};

/// Streamer manager with in-memory metadata and write-through persistence.
///
/// Database rows are authoritative. Manager mutations persist before updating
/// this cache; monitor and session transactions refresh it after committing.
pub struct StreamerManager<R>
where
    R: StreamerRepository + Send + Sync,
{
    /// In-memory metadata store.
    metadata: Arc<DashMap<String, Arc<StreamerMetadata>>>,
    cache: Arc<StreamerCache>,
    committed: Option<Arc<CommittedStreamerState>>,
    /// Lowercased URL index for fast lookups.
    url_index: Arc<DashMap<String, String>>,
    /// Monotonic revision fence for `apply_resolved_config_versioned` so
    /// slower concurrent config resolutions cannot regress `offline_check_*`.
    resolved_config_revisions: DashMap<String, u64>,
    /// Streamer repository for persistence.
    repo: Arc<R>,
    /// Event broadcaster for config updates.
    broadcaster: ConfigEventBroadcaster,
}

/// What [`StreamerManager::reload_from_repo`] announces about the row it just
/// read back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadPublish {
    /// Only the `StreamerStateSyncedFromDb` the reload already emits when the
    /// row's state or active status moved. For callers that changed a column no
    /// consumer routes on, and for pure cache resyncs after a transaction.
    StateOnly,
    /// Additionally publish `ConfigUpdateEvent::StreamerMetadataUpdated`, which
    /// makes `ActorRegistry::handle_config_event` re-resolve the actor's
    /// `ResolvedActorConfig` and `ConfigEventHandler` refresh the streamer's
    /// `offline_check_*` values. For callers that wrote the `streamers` row
    /// outside this manager.
    MetadataUpdated,
}

pub use srec_core::domain::streamer::StreamerUpdateParams;

impl<R> StreamerManager<R>
where
    R: StreamerRepository + Send + Sync,
{
    /// Create a new StreamerManager.
    pub fn new(repo: Arc<R>, broadcaster: ConfigEventBroadcaster) -> Self {
        let init_broadcaster = broadcaster.clone();
        let committed = repo
            .get_or_init_commit_owner(Box::new(move |write_pool| {
                Arc::new(CommittedStreamerState::new(
                    Arc::new(srec_sqlite::CommittedWriter::for_invalidation(
                        write_pool,
                        Arc::new(srec_core::utils::task_supervisor::TaskSupervisor::new()),
                    )),
                    init_broadcaster,
                ))
            }))
            .and_then(|owner| owner.as_any_arc().downcast::<CommittedStreamerState>().ok());
        let cache = committed
            .as_ref()
            .map(|state| state.cache.clone())
            .unwrap_or_else(|| Arc::new(StreamerCache::new(broadcaster.clone())));
        Self {
            metadata: cache.metadata.clone(),
            url_index: cache.urls.clone(),
            resolved_config_revisions: DashMap::new(),
            cache,
            committed,
            repo,
            broadcaster,
        }
    }

    // ========== Initialization ==========

    /// Hydrate the in-memory store from the database.
    ///
    /// This loads only the metadata fields needed for scheduling,
    /// avoiding full entity hydration for performance.
    ///
    /// **Restart Recovery**: Any streamers that were `Live` in the database are reset
    /// to `NotLive`. This ensures that after an app restart, the normal detection flow
    /// (`NotLive → Live`) will correctly trigger download starts. Without this reset,
    /// hysteresis would suppress `Live → Live` transitions and no download would start.
    ///
    /// Rows carrying a `deleted_at` marker are hydrated too, so
    /// [`StreamerMetadata::is_active`] keeps `ActorRegistry` from spawning an actor
    /// for a streamer whose retirement the reaper still has to finish; the query
    /// helpers below skip them.
    pub async fn hydrate(&self) -> Result<usize> {
        self.hydrate_with_recovery_status()
            .await
            .map(|(count, _)| count)
    }

    /// Hydrate best-effort scheduling state while reporting whether every
    /// persisted restart-state repair was confirmed.
    pub async fn hydrate_with_recovery_status(&self) -> Result<(usize, bool)> {
        if let Some(store) = &self.committed {
            return store
                .transaction("hydrate streamer cache", StatePublication::Silent, |tx| {
                    Box::pin(async move {
                        let rows = sqlx::query_as::<_, srec_sqlite::models::StreamerDbModel>(
                            "SELECT * FROM streamers ORDER BY priority DESC, name",
                        )
                        .fetch_all(&mut *tx)
                        .await?;
                        let complete = rows
                            .iter()
                            .all(|row| StreamerState::parse(&row.state).is_some());
                        let mut committed = Vec::with_capacity(rows.len());
                        for row in rows {
                            if row.state == StreamerState::Live.to_string() {
                                if let Some(row) =
                                    srec_sqlite::repositories::StreamerTxOps::set_offline_row(
                                        &mut *tx, &row.id,
                                    )
                                    .await?
                                {
                                    committed.push(row);
                                }
                            } else {
                                committed.push(row);
                            }
                        }
                        Ok(StateChange {
                            value: (committed.len(), complete),
                            rows: committed,
                            removed: Vec::new(),
                            reconfigured: Vec::new(),
                        })
                    })
                })
                .await;
        }

        info!("Hydrating streamer metadata from database");

        let streamers = self.repo.list_all_streamers().await?;
        let count = streamers.len();

        let mut live_reset_count = 0;
        let mut complete = true;

        for streamer in streamers {
            if StreamerState::parse(&streamer.state).is_none() {
                complete = false;
            }
            let mut metadata = StreamerMetadata::from_db_model(&streamer);

            // Restart recovery: reset Live to NotLive
            // After restart, no downloads are active, so we need the normal NotLive→Live
            // detection flow to work. If we keep Live state, hysteresis will suppress
            // the "redundant" Live→Live transition and no download will start.
            if metadata.state == StreamerState::Live {
                info!(
                    "Restart recovery: resetting streamer {} from Live to NotLive",
                    metadata.id
                );
                metadata.state = StreamerState::NotLive;
                live_reset_count += 1;

                // Persist the state change to keep DB consistent
                if let Err(e) = self
                    .repo
                    .update_streamer_state(&metadata.id, &StreamerState::NotLive.to_string())
                    .await
                {
                    complete = false;
                    warn!(
                        "Failed to persist NotLive state for streamer {} during restart recovery: {}",
                        metadata.id, e
                    );
                }
            }

            self.metadata
                .insert(metadata.id.clone(), Arc::new(metadata));
        }

        self.url_index.clear();
        for entry in self.metadata.iter() {
            self.url_index
                .insert(entry.url.to_lowercase(), entry.id.clone());
        }

        if live_reset_count > 0 {
            info!(
                "Restart recovery: reset {} streamers from Live to NotLive",
                live_reset_count
            );
        }

        info!("Hydrated {} streamers into memory", count);
        Ok((count, complete))
    }

    // ========== CRUD Operations (Write-Through) ==========

    /// Stores an identity only while its source and previous observation still match.
    /// Custom repositories without a committed writer do not persist observations.
    pub async fn observe_platform_metadata(
        &self,
        source: &StreamerMetadata,
        metadata: platforms_parser::media::PlatformMetadata,
        verified_at_ms: i64,
    ) -> Result<bool> {
        use platforms_parser::extractor::factory::{batch_probe_for_url, platform_name_for_url};

        let Some(store) = &self.committed else {
            return Ok(false);
        };
        if verified_at_ms <= 0
            || source.is_deleted()
            || metadata.schema_version != 1
            || !metadata.data.is_object()
            || platform_name_for_url(&source.url)
                .is_none_or(|name| name.to_ascii_lowercase() != metadata.platform)
        {
            return Ok(false);
        }
        // A probe that consumes identity must agree with the page that produced it.
        if batch_probe_for_url(&source.url)
            .is_some_and(|probe| probe.key_for_url(&source.url, Some(&metadata)).is_none())
        {
            return Ok(false);
        }
        let observation = super::platform_metadata::StoredPlatformMetadata {
            metadata,
            verified_at_ms,
        };
        if !observation.needs_write(source.platform_metadata.as_deref()) {
            return Ok(false);
        }
        let document = serde_json::to_string(&observation)?;
        let source = source.clone();
        store
            .transaction(
                "observe platform metadata",
                StatePublication::Silent,
                move |connection| {
                    Box::pin(async move {
                        let row = sqlx::query_as::<_, srec_sqlite::models::StreamerDbModel>(
                            "UPDATE streamers SET platform_metadata = ? \
                             WHERE id = ? AND url = ? AND platform_config_id = ? \
                             AND platform_metadata IS ? AND deleted_at IS NULL RETURNING *",
                        )
                        .bind(document)
                        .bind(&source.id)
                        .bind(&source.url)
                        .bind(&source.platform_config_id)
                        .bind(&source.platform_metadata)
                        .fetch_optional(connection)
                        .await?;
                        Ok(StateChange::row(row.is_some(), row))
                    })
                },
            )
            .await
    }

    /// Create a new streamer.
    ///
    /// Persists to database first, then updates in-memory cache.
    pub async fn create_streamer(&self, metadata: StreamerMetadata) -> Result<()> {
        debug!("Creating streamer: {}", metadata.id);

        // Convert to DB model and persist
        let db_model = self.metadata_to_db_model(&metadata);
        self.repo.create_streamer_for_manager(&db_model).await?;
        if self.committed.is_some() {
            return Ok(());
        }

        // Update in-memory cache
        self.metadata
            .insert(metadata.id.clone(), Arc::new(metadata.clone()));
        self.url_index
            .insert(metadata.url.to_lowercase(), metadata.id.clone());

        // Broadcast event
        self.broadcaster
            .publish(ConfigUpdateEvent::StreamerMetadataUpdated {
                streamer_id: metadata.id,
            });

        Ok(())
    }

    /// Update streamer priority.
    ///
    /// Persists to database first, then updates in-memory cache.
    pub async fn update_priority(&self, id: &str, priority: Priority) -> Result<()> {
        debug!("Updating priority for streamer {}: {:?}", id, priority);

        // Persist to database
        self.repo
            .update_streamer_priority(id, &priority.to_string())
            .await?;
        if self.committed.is_some() {
            return Ok(());
        }

        // Update in-memory cache
        if let Some(mut entry) = self.metadata.get_mut(id) {
            let entry = Arc::make_mut(entry.value_mut());
            entry.priority = priority;
        }

        self.broadcaster
            .publish(ConfigUpdateEvent::StreamerMetadataUpdated {
                streamer_id: id.to_owned(),
            });

        Ok(())
    }

    /// Clear error state for a streamer.
    ///
    /// Resets consecutive_error_count to 0 and clears disabled_until and
    /// last_error. An error state becomes NotLive; any other state, such as
    /// Disabled or Live, is kept.
    ///
    /// # Events
    /// Emits `ConfigUpdateEvent::StreamerErrorCleared` once the cache holds the
    /// cleared row, so `ActorRegistry` triggers an immediate check rather than waiting for the
    /// backoff deadline its actor already scheduled.
    pub async fn clear_error_state(&self, id: &str) -> Result<()> {
        debug!("Clearing error state for streamer {}", id);

        // Persist to database
        self.repo.clear_streamer_error_state(id).await?;

        if self.committed.is_none()
            && let Some(mut entry) = self.metadata.get_mut(id)
        {
            let entry = Arc::make_mut(entry.value_mut());
            entry.consecutive_error_count = 0;
            entry.disabled_until = None;
            entry.last_error = None;
            if entry.state.is_error() {
                entry.state = StreamerState::NotLive;
            }
        }

        self.broadcaster
            .publish(ConfigUpdateEvent::StreamerErrorCleared {
                streamer_id: id.to_owned(),
            });

        Ok(())
    }

    /// Clears the `last_error` field for the streamer with the given `id`.
    ///
    /// Only `last_error` is cleared; `consecutive_error_count` and `disabled_until`
    /// are left unchanged.
    pub async fn clear_last_error(&self, id: &str) -> Result<()> {
        debug!("Clearing last_error for streamer {}", id);

        self.repo.clear_streamer_last_error(id).await?;
        if self.committed.is_some() {
            return Ok(());
        }

        if let Some(mut entry) = self.metadata.get_mut(id) {
            let entry = Arc::make_mut(entry.value_mut());
            entry.last_error = None;
        }

        Ok(())
    }

    /// Update streamer state.
    ///
    /// Persists to database first, then updates in-memory cache.
    pub async fn update_state(&self, id: &str, state: StreamerState) -> Result<()> {
        debug!("Updating state for streamer {}: {:?}", id, state);

        // Persist to database
        self.repo
            .update_streamer_state(id, &state.to_string())
            .await?;
        if self.committed.is_some() {
            return Ok(());
        }

        // Update in-memory cache
        if let Some(mut entry) = self.metadata.get_mut(id) {
            let entry = Arc::make_mut(entry.value_mut());
            entry.state = state;
        }

        Ok(())
    }

    /// Reload streamer metadata from the repository into the in-memory cache.
    ///
    /// This is useful when another component performs transactional DB updates
    /// (e.g., `RecorderStore` session/state transactions) and we need to bring
    /// the in-memory cache back in sync.
    ///
    /// # Transactional Update Pattern
    ///
    /// When performing transactional updates that modify streamer state:
    /// 1. Use `StreamerTxOps` methods within a transaction
    /// 2. Commit the transaction
    /// 3. Call `reload_from_repo()` to sync the in-memory cache
    ///
    /// This ensures the in-memory cache reflects the committed DB state.
    ///
    /// # Event Emission
    ///
    /// Emits `ConfigUpdateEvent::StreamerStateSyncedFromDb` ONLY if the
    /// streamer's active status or state actually changed, which prevents event
    /// spam when no real change occurred. [`ReloadPublish::MetadataUpdated`]
    /// adds `ConfigUpdateEvent::StreamerMetadataUpdated` on top, for callers
    /// that wrote the row themselves and need the same observable sequence as
    /// [`Self::create_streamer`] / [`Self::partial_update_streamer`].
    ///
    /// Resolved offline-check values are preserved because they live only in
    /// the runtime cache rather than on the streamer database row.
    pub async fn reload_from_repo(
        &self,
        id: &str,
        publish: ReloadPublish,
    ) -> Result<Option<StreamerMetadata>> {
        if let Some(state) = &self.committed {
            state
                .reload(
                    id,
                    if publish == ReloadPublish::MetadataUpdated {
                        StatePublication::Metadata
                    } else {
                        StatePublication::StateOnly
                    },
                )
                .await?;
            return Ok(self.get_streamer(id));
        }
        // Preserve resolved runtime-only configuration, which is not stored
        // on the streamer row being reloaded.
        let old_state = self.metadata.get(id).map(|e| {
            (
                e.state,
                e.is_active(),
                e.offline_check_count,
                e.offline_check_delay_ms,
                e.url.to_lowercase(),
            )
        });

        match self.repo.get_streamer(id).await {
            Ok(model) => {
                let mut metadata = StreamerMetadata::from_db_model(&model);
                if let Some((_, _, offline_check_count, offline_check_delay_ms, _)) = &old_state {
                    metadata.offline_check_count = *offline_check_count;
                    metadata.offline_check_delay_ms = *offline_check_delay_ms;
                }
                let new_is_active = metadata.is_active();
                let new_state = metadata.state;
                // Keep `url_index` in step with `metadata` so `get_streamer_by_url`
                // resolves rows this reload introduced or re-pointed.
                let new_url = metadata.url.to_lowercase();
                if let Some((_, _, _, _, old_url)) = &old_state
                    && old_url != &new_url
                {
                    self.url_index.remove(old_url);
                }
                self.url_index.insert(new_url, id.to_string());
                self.metadata
                    .insert(id.to_string(), Arc::new(metadata.clone()));

                // Only emit event if state or active status actually changed
                let should_emit = match &old_state {
                    Some((old_s, old_active, _, _, _)) => {
                        old_s != &new_state || old_active != &new_is_active
                    }
                    None => true, // New entry, always emit
                };

                if should_emit {
                    debug!(
                        "Streamer {} state changed: {:?} -> {:?} (active: {})",
                        id,
                        old_state.as_ref().map(|(state, _, _, _, _)| *state),
                        new_state,
                        new_is_active
                    );
                    self.broadcaster
                        .publish(ConfigUpdateEvent::StreamerStateSyncedFromDb {
                            streamer_id: id.to_string(),
                            is_active: new_is_active,
                        });
                }

                if publish == ReloadPublish::MetadataUpdated {
                    self.broadcaster
                        .publish(ConfigUpdateEvent::StreamerMetadataUpdated {
                            streamer_id: id.to_string(),
                        });
                }

                Ok(Some(metadata))
            }
            Err(srec_sqlite::DbError::NotFound { .. }) => {
                let removed = self.metadata.remove(id);
                if let Some((_, entry)) = &removed {
                    self.url_index.remove(&entry.url.to_lowercase());
                }

                // Only emit if we actually removed something
                if removed.is_some() {
                    self.broadcaster
                        .publish(ConfigUpdateEvent::StreamerStateSyncedFromDb {
                            streamer_id: id.to_string(),
                            is_active: false,
                        });
                }

                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Update a streamer's avatar.
    pub async fn update_avatar(&self, id: &str, avatar_url: Option<String>) -> Result<()> {
        debug!("Updating avatar for streamer {}", id);

        // Persist to database
        self.repo.update_avatar(id, avatar_url.as_deref()).await?;
        if self.committed.is_some() {
            return Ok(());
        }

        // Update in-memory cache
        if let Some(mut entry) = self.metadata.get_mut(id) {
            let entry = Arc::make_mut(entry.value_mut());
            entry.avatar_url = avatar_url;
        }

        Ok(())
    }

    /// Partially update a streamer.
    ///
    /// Only updates the fields that are provided (Some values).
    /// Persists to database first, then updates in-memory cache.
    ///
    /// A `state` of [`StreamerState::Disabled`] also clears the error history
    /// and backoff in the same write, so a re-enable can check immediately.
    ///
    /// # Events
    /// Always emits `ConfigUpdateEvent::StreamerMetadataUpdated`, including when the update changes
    /// the streamer's state (e.g., user disables a streamer). `StreamerStateSyncedFromDb` is reserved
    /// for transactional DB sync via `reload_from_repo()`.
    pub async fn partial_update_streamer(
        &self,
        params: StreamerUpdateParams,
    ) -> Result<StreamerMetadata> {
        let id = params.id.clone();
        let model = self.repo.patch_streamer_for_manager(params).await?;
        if self.committed.is_none() {
            self.cache
                .apply(&StateChange::row((), Some(model)), StatePublication::Silent);
        }
        if self.committed.is_none() {
            self.broadcaster
                .publish(ConfigUpdateEvent::StreamerMetadataUpdated {
                    streamer_id: id.clone(),
                });
        }
        self.get_streamer(&id)
            .ok_or_else(|| crate::Error::not_found("Streamer", id))
    }

    /// Mark a streamer deleted, so every runtime owner stands down before the
    /// row is removed.
    ///
    /// Writes `streamers.deleted_at` and reloads the row, which turns
    /// [`StreamerMetadata::is_active`] false and publishes
    /// `StreamerStateSyncedFromDb { is_active: false }`. Stopping the actor
    /// and physical owners belongs to `ActorRegistry::retire_streamer`, which
    /// gates [`Self::reap_deleted`] on all owners having settled.
    ///
    /// Returns the marked metadata, or `None` when the row is missing or was
    /// already marked by an earlier caller that owns its retirement.
    pub async fn mark_deleting(&self, id: &str) -> Result<Option<StreamerMetadata>> {
        if !self.repo.mark_streamer_deleted(id).await? {
            debug!("Streamer {} is missing or already marked deleted", id);
            return Ok(None);
        }

        if self.committed.is_some() {
            return Ok(self.get_streamer(id));
        }
        info!("Marked streamer {} deleted; retiring its runtime work", id);
        self.reload_from_repo(id, ReloadPublish::StateOnly).await
    }

    /// Delete a marked streamer's row and drop it from the caches.
    ///
    /// Refuses rows without a `deleted_at` marker, so the only way to remove a
    /// streamer is [`Self::mark_deleting`] followed by a retirement that
    /// acknowledged. Broadcasts `StreamerDeleted` so `ActorRegistry` drops any
    /// actor that outlived the mark and `ConfigService` evicts its cache entry.
    ///
    /// `Ok(false)` when the row was already reaped.
    pub async fn reap_deleted(&self, id: &str) -> Result<bool> {
        let reaped = self.repo.delete_marked_streamer(id).await?;
        if reaped {
            self.resolved_config_revisions.remove(id);
        }
        if self.committed.is_some() {
            return Ok(reaped);
        }

        if let Some((_, entry)) = self.metadata.remove(id) {
            self.url_index.remove(&entry.url.to_lowercase());
        }

        self.broadcaster
            .publish(ConfigUpdateEvent::StreamerDeleted {
                streamer_id: id.to_string(),
            });

        if reaped {
            info!("Reaped deleted streamer {}", id);
        }
        Ok(reaped)
    }

    /// Metadata of every streamer carrying a `deleted_at` marker.
    ///
    /// Read from the cache rather than the repository, so it reflects the same
    /// rows the runtime is standing down. The cache holds every marked row:
    /// [`Self::hydrate`] loads them all through
    /// `StreamerRepository::list_all_streamers`, and
    /// `ServiceContainer::initialize` propagates a hydration failure, so there
    /// is no partially-populated state in which a marker could be missed.
    pub fn get_pending_deletion(&self) -> Vec<StreamerMetadata> {
        self.metadata
            .iter()
            .filter(|entry| entry.is_deleted())
            .map(|entry| entry.value().as_ref().clone())
            .collect()
    }

    /// Refresh the cached `offline_check_*` values on a streamer's metadata
    /// from a freshly resolved [`crate::config::MergedConfig`].
    /// No-op if the streamer is not currently registered.
    ///
    /// Called from `ActorRegistry`'s config resolution and `ConfigEventHandler`
    /// so per-streamer overrides stay current in cached metadata and
    /// `StreamerFsm`'s quiet-period backstop without an extra DB hit.
    pub fn apply_resolved_config(&self, streamer_id: &str, merged: &crate::config::MergedConfig) {
        let _publication = self.cache.publication.write();
        if let Some(mut entry) = self.metadata.get_mut(streamer_id) {
            let entry = Arc::make_mut(entry.value_mut());
            entry.apply_resolved_config(merged);
        }
    }

    /// Revision-fenced variant of [`Self::apply_resolved_config`]: ignores a
    /// stale async resolution whose pre-allocated `revision` is older than the
    /// last applied revision for `streamer_id`.
    pub fn apply_resolved_config_versioned(
        &self,
        streamer_id: &str,
        revision: u64,
        merged: &crate::config::MergedConfig,
    ) {
        let _publication = self.cache.publication.write();
        if let Some(last) = self.resolved_config_revisions.get(streamer_id)
            && revision < *last
        {
            return;
        }
        if let Some(mut entry) = self.metadata.get_mut(streamer_id) {
            let entry = Arc::make_mut(entry.value_mut());
            entry.apply_resolved_config(merged);
            self.resolved_config_revisions
                .insert(streamer_id.to_owned(), revision);
        }
    }

    // ========== Query Operations (From Memory) ==========

    /// Get streamer metadata by ID, including one marked deleted.
    ///
    /// Marked rows are deliberately visible here: every runtime owner reads this
    /// and decides on [`StreamerMetadata::is_active`], and
    /// `run_live_download_pipeline` starts a recording when it finds no metadata
    /// at all. Callers acting on the user's behalf reject
    /// [`StreamerMetadata::is_deleted`] themselves; the bulk queries below
    /// already do.
    pub fn get_streamer_snapshot(&self, id: &str) -> Option<Arc<StreamerMetadata>> {
        self.metadata.get(id).map(|entry| entry.value().clone())
    }

    /// Import publishes its notifications after releasing the database lease.
    /// Read the current snapshot under the same short publication lock so an
    /// intervening write cannot receive an older active/inactive payload.
    pub fn publish_imported(
        &self,
        id: &str,
        state_changed: bool,
        metadata_changed: bool,
    ) -> Option<StreamerMetadata> {
        let _publication = self.cache.publication.read();
        let metadata = self.get_streamer(id)?;
        if state_changed {
            self.broadcaster
                .publish(ConfigUpdateEvent::StreamerStateSyncedFromDb {
                    streamer_id: id.to_owned(),
                    is_active: metadata.is_active(),
                });
        }
        if metadata_changed {
            self.broadcaster
                .publish(ConfigUpdateEvent::StreamerMetadataUpdated {
                    streamer_id: id.to_owned(),
                });
        }
        Some(metadata)
    }

    pub fn committed_state(&self) -> Option<Arc<CommittedStreamerState>> {
        self.committed.clone()
    }

    pub fn get_streamer(&self, id: &str) -> Option<StreamerMetadata> {
        self.metadata
            .get(id)
            .map(|entry| entry.value().as_ref().clone())
    }

    /// Get streamer metadata by URL (case-insensitive).
    ///
    /// Includes a streamer marked deleted, so the URL a pending retirement still
    /// holds resolves to the row that holds it rather than to nothing.
    pub fn get_streamer_by_url(&self, url: &str) -> Option<StreamerMetadata> {
        let _publication = self.cache.publication.read();
        let url_lower = url.to_lowercase();
        let id = self.url_index.get(&url_lower)?;
        self.get_streamer(id.value())
    }

    /// Get all streamers the user still has, skipping ones marked deleted.
    pub fn get_all(&self) -> Vec<StreamerMetadata> {
        self.get_filtered(|_| true)
    }

    /// Clone matching streamer metadata, skipping streamers marked deleted.
    fn get_filtered(&self, predicate: impl Fn(&StreamerMetadata) -> bool) -> Vec<StreamerMetadata> {
        self.collect_filtered(predicate, |metadata| metadata.as_ref().clone())
    }

    /// Share the snapshots of matching streamers, skipping streamers marked
    /// deleted. No metadata is copied, which suits a caller that sorts every
    /// match and reads only some of them, as a paged list does.
    pub fn get_filtered_snapshots(
        &self,
        predicate: impl Fn(&StreamerMetadata) -> bool,
    ) -> Vec<Arc<StreamerMetadata>> {
        self.collect_filtered(predicate, Arc::clone)
    }

    fn collect_filtered<T>(
        &self,
        predicate: impl Fn(&StreamerMetadata) -> bool,
        project: impl Fn(&Arc<StreamerMetadata>) -> T,
    ) -> Vec<T> {
        self.metadata
            .iter()
            .filter(|entry| !entry.is_deleted() && predicate(entry.value()))
            .map(|entry| project(entry.value()))
            .collect()
    }

    /// Get all active streamers.
    ///
    /// Returns streamers in active states (NotLive, Live, OutOfSchedule, InspectingLive).
    pub fn get_all_active(&self) -> Vec<StreamerMetadata> {
        self.get_filtered(|metadata| metadata.is_active())
    }

    /// Get streamers by platform.
    pub fn get_by_platform(&self, platform_id: &str) -> Vec<StreamerMetadata> {
        self.get_filtered(|metadata| metadata.platform_config_id == platform_id)
    }

    /// Get streamers by template.
    pub fn get_by_template(&self, template_id: &str) -> Vec<StreamerMetadata> {
        self.get_filtered(|metadata| metadata.template_config_id.as_deref() == Some(template_id))
    }

    /// Number of `streamers` rows referencing `template_id`, including ones
    /// marked deleted.
    ///
    /// `streamers.template_config_id` is a foreign key with no `ON DELETE`
    /// action, so any row still in the table blocks
    /// `DELETE FROM template_config` whether or not the user can still see it.
    /// [`Self::get_by_template`] answers the narrower "streamers the user has"
    /// question the usage counts report.
    pub fn template_reference_count(&self, template_id: &str) -> usize {
        self.metadata
            .iter()
            .filter(|entry| entry.template_config_id.as_deref() == Some(template_id))
            .count()
    }

    /// Check if a streamer is currently disabled.
    pub fn is_disabled(&self, id: &str) -> bool {
        self.metadata
            .get(id)
            .map(|entry| entry.is_disabled())
            .unwrap_or(false)
    }

    /// Get a reference to the underlying metadata store.
    ///
    /// This is useful for actors that need direct read access to streamer metadata
    /// without going through the manager's methods. The returned Arc can be cloned
    /// and shared with actors for efficient metadata lookups.
    pub fn metadata_store(&self) -> Arc<DashMap<String, Arc<StreamerMetadata>>> {
        self.metadata.clone()
    }

    // ========== URL Uniqueness Checks ==========

    /// Check if a URL exists for any streamer other than the specified one.
    ///
    /// Used during updates to allow a streamer to keep its own URL.
    /// Performs case-insensitive comparison.
    pub fn url_exists_for_other(&self, url: &str, exclude_id: &str) -> bool {
        let url_lower = url.to_lowercase();
        self.url_index
            .get(&url_lower)
            .map(|entry| entry.value() != exclude_id)
            .unwrap_or(false)
    }

    // ========== Private Helpers ==========

    /// Convert metadata to database model.
    fn metadata_to_db_model(
        &self,
        metadata: &StreamerMetadata,
    ) -> srec_sqlite::models::StreamerDbModel {
        srec_sqlite::models::StreamerDbModel {
            id: metadata.id.clone(),
            name: metadata.name.clone(),
            url: metadata.url.clone(),
            platform_config_id: metadata.platform_config_id.clone(),
            template_config_id: metadata.template_config_id.clone(),
            state: metadata.state.to_string(),
            priority: metadata.priority.to_string(),
            avatar: metadata.avatar_url.clone(),
            consecutive_error_count: Some(metadata.consecutive_error_count),
            last_error: metadata.last_error.clone(),
            disabled_until: metadata.disabled_until.map(|dt| dt.as_millisecond()),
            last_live_time: metadata.last_live_time.map(|dt| dt.as_millisecond()),
            streamer_specific_config: metadata.streamer_specific_config.clone(),
            platform_metadata: metadata.platform_metadata.clone(),
            created_at: metadata.created_at.as_millisecond(),
            updated_at: metadata.updated_at.as_millisecond(),
            deleted_at: metadata.deleted_at.map(|dt| dt.as_millisecond()),
        }
    }
}
