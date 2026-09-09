//! Committed streamer rows and the runtime's immutable metadata snapshots.

use std::sync::Arc;

use dashmap::DashMap;
use futures::future::BoxFuture;
use parking_lot::RwLock;
use sqlx::SqliteConnection;

use super::StreamerMetadata;
use crate::Result;
use crate::config::{ConfigEventBroadcaster, ConfigUpdateEvent};
use crate::database::{CommittedWriter, models::StreamerDbModel};

#[derive(Clone, Copy)]
pub(crate) enum StatePublication {
    Silent,
    StateOnly,
    Metadata,
    Manager,
    Deleted,
}

pub(crate) struct StateChange<T> {
    pub value: T,
    pub rows: Vec<StreamerDbModel>,
    pub removed: Vec<String>,
}

impl<T> StateChange<T> {
    pub(crate) fn row(value: T, row: Option<StreamerDbModel>) -> Self {
        Self {
            value,
            rows: row.into_iter().collect(),
            removed: Vec::new(),
        }
    }
}

type RemovalCallback = Arc<dyn Fn(&str) + Send + Sync>;

pub(crate) struct StreamerCache {
    pub metadata: Arc<DashMap<String, Arc<StreamerMetadata>>>,
    pub urls: Arc<DashMap<String, String>>,
    pub publication: RwLock<()>,
    broadcaster: ConfigEventBroadcaster,
    on_removed: RwLock<Option<RemovalCallback>>,
    on_changed: RwLock<Option<RemovalCallback>>,
}

impl StreamerCache {
    pub(crate) fn new(broadcaster: ConfigEventBroadcaster) -> Self {
        Self {
            metadata: Arc::new(DashMap::new()),
            urls: Arc::new(DashMap::new()),
            publication: RwLock::new(()),
            broadcaster,
            on_removed: RwLock::new(None),
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
            let config_changed = old.as_ref().is_none_or(|old| {
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
            if let Some(invalidate) = self.on_removed.read().as_ref() {
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
}

impl CommittedStreamerState {
    pub(crate) fn new(writer: Arc<CommittedWriter>, broadcaster: ConfigEventBroadcaster) -> Self {
        Self {
            writer,
            cache: Arc::new(StreamerCache::new(broadcaster)),
        }
    }

    pub(crate) fn with_cache(writer: Arc<CommittedWriter>, cache: Arc<StreamerCache>) -> Self {
        Self { writer, cache }
    }

    /// Called synchronously under the cache publication lock, before any event.
    /// The callback may invalidate other caches but must not re-enter streamer
    /// cache access, await work, or attempt to acquire the database writer.
    pub(crate) fn on_removed(&self, callback: Arc<dyn Fn(&str) + Send + Sync>) {
        *self.cache.on_removed.write() = Some(callback);
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
        self.writer
            .transaction(label, operation, move |change| {
                cache.apply(change, publication)
            })
            .await
            .map(|change| change.value)
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

#[cfg(test)]
mod tests;
