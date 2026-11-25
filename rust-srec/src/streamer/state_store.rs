//! Committed streamer rows and the runtime's immutable metadata snapshots.

use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use futures::future::BoxFuture;
use parking_lot::RwLock;
use sqlx::{SqliteConnection, SqlitePool};

use super::StreamerMetadata;
use crate::Result;
use crate::config::{ConfigEventBroadcaster, ConfigUpdateEvent};
pub(crate) use crate::database::repositories::streamer::{
    StateChange, StatePublication, StreamerCommitOwner, StreamerTxOp,
};
use crate::database::{CommittedWriter, models::StreamerDbModel};

type RemovalCallback = Arc<dyn Fn(&str) + Send + Sync>;

pub(crate) struct StreamerCache {
    pub metadata: Arc<DashMap<String, Arc<StreamerMetadata>>>,
    pub urls: Arc<DashMap<String, String>>,
    pub publication: RwLock<()>,
    broadcaster: ConfigEventBroadcaster,
    on_removed: RwLock<Vec<RemovalCallback>>,
    on_changed: RwLock<Option<RemovalCallback>>,
}

impl StreamerCache {
    pub(crate) fn new(broadcaster: ConfigEventBroadcaster) -> Self {
        Self {
            metadata: Arc::new(DashMap::new()),
            urls: Arc::new(DashMap::new()),
            publication: RwLock::new(()),
            broadcaster,
            on_removed: RwLock::new(Vec::new()),
            on_changed: RwLock::new(None),
        }
    }

    pub(crate) fn apply<T>(&self, change: &StateChange<T>, publication: StatePublication) {
        let _publication = self.publication.write();
        for model in &change.rows {
            let old = self
                .metadata
                .get(&model.id)
                .map(|entry| entry.value().clone());
            let mut next = StreamerMetadata::from_db_model(model);
            if let Some(old) = &old {
                next.offline_check_count = old.offline_check_count;
                next.offline_check_delay_ms = old.offline_check_delay_ms;
                if old.url.to_lowercase() != next.url.to_lowercase() {
                    self.urls
                        .remove_if(&old.url.to_lowercase(), |_, id| id == &model.id);
                }
            }
            let state_changed = old
                .as_ref()
                .is_none_or(|old| old.state != next.state || old.is_active() != next.is_active());
            let active = next.is_active();
            let config_changed = change.reconfigured.contains(&model.id)
                || old.as_ref().is_none_or(|old| {
                    old.name != next.name
                        || old.url != next.url
                        || old.platform_config_id != next.platform_config_id
                        || old.template_config_id != next.template_config_id
                        || old.streamer_specific_config != next.streamer_specific_config
                });
            self.urls.insert(next.url.to_lowercase(), next.id.clone());
            self.metadata.insert(next.id.clone(), Arc::new(next));
            if config_changed && let Some(invalidate) = self.on_changed.read().as_ref() {
                invalidate(&model.id);
            }
            if state_changed
                && matches!(
                    publication,
                    StatePublication::StateOnly | StatePublication::Metadata
                )
            {
                self.broadcaster
                    .publish(ConfigUpdateEvent::StreamerStateSyncedFromDb {
                        streamer_id: model.id.clone(),
                        is_active: active,
                    });
            }
            if matches!(
                publication,
                StatePublication::Metadata | StatePublication::Manager
            ) {
                self.broadcaster
                    .publish(ConfigUpdateEvent::StreamerMetadataUpdated {
                        streamer_id: model.id.clone(),
                    });
            }
        }
        for id in &change.removed {
            let removed = self.metadata.remove(id);
            if let Some((_, old)) = &removed {
                self.urls
                    .remove_if(&old.url.to_lowercase(), |_, owner| owner == id);
            }
            for invalidate in self.on_removed.read().iter() {
                invalidate(id);
            }
            match publication {
                StatePublication::Deleted => {
                    self.broadcaster
                        .publish(ConfigUpdateEvent::StreamerDeleted {
                            streamer_id: id.clone(),
                        });
                }
                StatePublication::StateOnly | StatePublication::Metadata if removed.is_some() => {
                    self.broadcaster
                        .publish(ConfigUpdateEvent::StreamerStateSyncedFromDb {
                            streamer_id: id.clone(),
                            is_active: false,
                        });
                }
                _ => {}
            }
        }
    }
}

/// Shared row publication owner used by the concrete repositories and runtime.
pub struct CommittedStreamerState {
    pub(crate) writer: Arc<CommittedWriter>,
    pub(crate) cache: Arc<StreamerCache>,
    post_publication: RwLock<Vec<Arc<dyn crate::recorder::PostPublicationListener>>>,
}

impl CommittedStreamerState {
    /// Return the latest published metadata snapshot for `id`, if cached.
    pub fn get_streamer(&self, id: &str) -> Option<Arc<StreamerMetadata>> {
        let _publication = self.cache.publication.read();
        self.cache
            .metadata
            .get(id)
            .map(|entry| Arc::clone(entry.value()))
    }

    pub(crate) fn new(writer: Arc<CommittedWriter>, broadcaster: ConfigEventBroadcaster) -> Self {
        Self {
            writer,
            cache: Arc::new(StreamerCache::new(broadcaster)),
            post_publication: RwLock::new(Vec::new()),
        }
    }

    /// Register an asynchronous listener invoked strictly after both
    /// `StreamerCache::publication` and the SQLite write connection have been
    /// released.
    pub(crate) fn add_post_publication_listener(
        &self,
        listener: Arc<dyn crate::recorder::PostPublicationListener>,
    ) {
        self.post_publication.write().push(listener);
    }

    /// Notify registered post-publication listeners outside any cache lock or
    /// database write transaction.
    pub(crate) async fn notify_post_publication(
        &self,
        changes: crate::recorder::PostPublicationChangeSet,
    ) {
        if changes.is_empty() {
            return;
        }
        let listeners = self.post_publication.read().clone();
        for listener in listeners {
            listener.on_published(changes.clone()).await;
        }
    }

    /// Called synchronously under the cache publication lock, before any event.
    /// The callback may invalidate other caches but must not re-enter streamer
    /// cache access, await work, or attempt to acquire the database writer.
    pub(crate) fn on_removed(&self, callback: Arc<dyn Fn(&str) + Send + Sync>) {
        self.cache.on_removed.write().push(callback);
    }

    /// Synchronous, non-reentrant invalidation with the same locking contract as
    /// on_removed. State/error-only changes do not invalidate merged configuration.
    pub(crate) fn on_changed(&self, callback: RemovalCallback) {
        *self.cache.on_changed.write() = Some(callback);
    }

    pub(crate) async fn transaction<T, F>(
        &self,
        label: &'static str,
        publication: StatePublication,
        operation: F,
    ) -> Result<T>
    where
        T: Send + 'static,
        F: for<'c> FnOnce(&'c mut SqliteConnection) -> BoxFuture<'c, Result<StateChange<T>>>
            + Send
            + 'static,
    {
        self.writer.require_serialized()?;
        let cache = self.cache.clone();
        let change = self
            .writer
            .transaction(label, operation, move |change| {
                cache.apply(change, publication)
            })
            .await?;
        let value = change.value;
        if !matches!(publication, StatePublication::Silent) {
            let change_set = crate::recorder::PostPublicationChangeSet {
                updated_streamers: change.rows.into_iter().map(|row| row.id).collect(),
                removed_streamers: change.removed,
                reconfigured_streamers: change.reconfigured,
                outbox_enqueued: false,
            };
            self.notify_post_publication(change_set).await;
        }
        Ok(value)
    }

    pub(crate) async fn reload(
        &self,
        id: &str,
        publication: StatePublication,
    ) -> Result<Option<StreamerMetadata>> {
        let id = id.to_owned();
        self.transaction(
            "reload committed streamer",
            publication,
            move |connection| {
                Box::pin(async move {
                    let row = sqlx::query_as::<_, StreamerDbModel>(
                        "SELECT * FROM streamers WHERE id = ?",
                    )
                    .bind(&id)
                    .fetch_optional(connection)
                    .await?;
                    let value = row.as_ref().map(StreamerMetadata::from_db_model);
                    let mut change = StateChange::row(value, row);
                    if change.rows.is_empty() {
                        change.removed.push(id);
                    }
                    Ok(change)
                })
            },
        )
        .await
    }
}

#[async_trait]
impl StreamerCommitOwner for CommittedStreamerState {
    fn require_same_pool(&self, pool: &SqlitePool) -> Result<()> {
        self.writer.require_same_pool(pool)
    }

    async fn commit_streamer_write(
        &self,
        label: &'static str,
        publication: StatePublication,
        operation: StreamerTxOp,
    ) -> Result<Option<StreamerDbModel>> {
        self.transaction(label, publication, operation).await
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }
}

#[cfg(test)]
mod tests;
