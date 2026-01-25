//! Streamer manager implementation.
//!
//! The StreamerManager maintains in-memory streamer metadata with
//! write-through persistence to the database.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use tracing::{debug, info, warn};

use crate::Result;
use crate::config::{ConfigEventBroadcaster, ConfigUpdateEvent};
use crate::database::repositories::streamer::StreamerRepository;
use crate::domain::{Priority, StreamerState};

use super::metadata::StreamerMetadata;

/// Default error threshold before applying backoff.
const DEFAULT_ERROR_THRESHOLD: i32 = 3;

/// Base backoff duration (doubles with each error).
const BASE_BACKOFF_SECS: u64 = 60;

/// Maximum backoff duration (1 hour).
const MAX_BACKOFF_SECS: u64 = 3600;

/// Streamer manager with in-memory metadata and write-through persistence.
///
/// This is the single source of truth for streamer state during runtime.
/// All state changes are persisted to the database before updating memory.
pub struct StreamerManager<R>
where
    R: StreamerRepository + Send + Sync,
{
    /// In-memory metadata store.
    metadata: Arc<DashMap<String, StreamerMetadata>>,
    /// Lowercased URL index for fast lookups.
    url_index: Arc<DashMap<String, String>>,
    /// Streamer repository for persistence.
    repo: Arc<R>,
    /// Event broadcaster for config updates.
    broadcaster: ConfigEventBroadcaster,
    /// Error threshold before backoff.
    error_threshold: i32,
}

/// Parameters for partially updating a streamer.
pub struct StreamerUpdateParams {
    pub id: String,
    pub name: Option<String>,
    pub url: Option<String>,
    pub template_config_id: Option<Option<String>>,
    pub priority: Option<Priority>,
    pub state: Option<StreamerState>,
    pub streamer_specific_config: Option<Option<String>>,
}

impl<R> StreamerManager<R>
where
    R: StreamerRepository + Send + Sync,
{
    /// Create a new StreamerManager.
    pub fn new(repo: Arc<R>, broadcaster: ConfigEventBroadcaster) -> Self {
        Self {
            metadata: Arc::new(DashMap::new()),
            url_index: Arc::new(DashMap::new()),
            repo,
            broadcaster,
            error_threshold: DEFAULT_ERROR_THRESHOLD,
        }
    }

    /// Create a new StreamerManager with custom error threshold.
    pub fn with_error_threshold(
        repo: Arc<R>,
        broadcaster: ConfigEventBroadcaster,
        error_threshold: i32,
    ) -> Self {
        Self {
            metadata: Arc::new(DashMap::new()),
            url_index: Arc::new(DashMap::new()),
            repo,
            broadcaster,
            error_threshold,
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
    pub async fn hydrate(&self) -> Result<usize> {
        info!("Hydrating streamer metadata from database");

        let streamers = self.repo.list_all_streamers().await?;
        let count = streamers.len();

        let mut live_reset_count = 0;

        for streamer in streamers {
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
                    warn!(
                        "Failed to persist NotLive state for streamer {} during restart recovery: {}",
                        metadata.id, e
                    );
                }
            }

            self.metadata.insert(metadata.id.clone(), metadata);
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
        Ok(count)
    }

    // ========== CRUD Operations (Write-Through) ==========

    /// Create a new streamer.
    ///
    /// Persists to database first, then updates in-memory cache.
    pub async fn create_streamer(&self, metadata: StreamerMetadata) -> Result<()> {
        debug!("Creating streamer: {}", metadata.id);

        // Convert to DB model and persist
        let db_model = self.metadata_to_db_model(&metadata);
        self.repo.create_streamer(&db_model).await?;

        // Update in-memory cache
        self.metadata.insert(metadata.id.clone(), metadata.clone());
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

        // Update in-memory cache
        if let Some(mut entry) = self.metadata.get_mut(id) {
            entry.priority = priority;
        }

        Ok(())
    }

    /// Clear error state for a streamer.
    ///
    /// Resets consecutive_error_count to 0, clears disabled_until,
    /// and sets state to NotLive.
    pub async fn clear_error_state(&self, id: &str) -> Result<()> {
        debug!("Clearing error state for streamer {}", id);

        // Persist to database
        self.repo.clear_streamer_error_state(id).await?;

        // Update in-memory cache
        if let Some(mut entry) = self.metadata.get_mut(id) {
            entry.consecutive_error_count = 0;
            entry.disabled_until = None;
            entry.last_error = None;
            entry.state = StreamerState::NotLive;
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

        // Update in-memory cache
        if let Some(mut entry) = self.metadata.get_mut(id) {
            entry.state = state;
        }

        Ok(())
    }

    /// Reload streamer metadata from the repository into the in-memory cache.
    ///
    /// This is useful when another component performs transactional DB updates
    /// (e.g., StreamMonitor session/state transactions) and we need to bring
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
    /// This method emits a `ConfigUpdateEvent::StreamerStateSyncedFromDb` event
    /// ONLY if the streamer's active status or state actually changed.
    /// This prevents unnecessary event spam when no real change occurred.
    pub async fn reload_from_repo(&self, id: &str) -> Result<Option<StreamerMetadata>> {
        // Capture old state before reload
        let old_state = self.metadata.get(id).map(|e| (e.state, e.is_active()));

        match self.repo.get_streamer(id).await {
            Ok(model) => {
                let metadata = StreamerMetadata::from_db_model(&model);
                let new_is_active = metadata.is_active();
                let new_state = metadata.state;
                self.metadata.insert(id.to_string(), metadata.clone());

                // Only emit event if state or active status actually changed
                let should_emit = match old_state {
                    Some((old_s, old_active)) => old_s != new_state || old_active != new_is_active,
                    None => true, // New entry, always emit
                };

                if should_emit {
                    debug!(
                        "Streamer {} state changed: {:?} -> {:?} (active: {})",
                        id,
                        old_state.map(|(s, _)| s),
                        new_state,
                        new_is_active
                    );
                    self.broadcaster
                        .publish(ConfigUpdateEvent::StreamerStateSyncedFromDb {
                            streamer_id: id.to_string(),
                            is_active: new_is_active,
                        });
                }

                Ok(Some(metadata))
            }
            Err(crate::Error::NotFound { .. }) => {
                let was_present = self.metadata.remove(id).is_some();

                // Only emit if we actually removed something
                if was_present {
                    self.broadcaster
                        .publish(ConfigUpdateEvent::StreamerStateSyncedFromDb {
                            streamer_id: id.to_string(),
                            is_active: false,
                        });
                }

                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Reload multiple streamers from the repository into the in-memory cache.
    ///
    /// This is more efficient than calling `reload_from_repo` multiple times
    /// when multiple streamers need to be synced after a batch transaction.
    pub async fn reload_multiple_from_repo(&self, ids: &[&str]) -> Result<usize> {
        let mut reloaded = 0;
        for id in ids {
            if self.reload_from_repo(id).await?.is_some() {
                reloaded += 1;
            }
        }
        Ok(reloaded)
    }

    /// Update a streamer's avatar.
    pub async fn update_avatar(&self, id: &str, avatar_url: Option<String>) -> Result<()> {
        debug!("Updating avatar for streamer {}", id);

        // Persist to database
        self.repo.update_avatar(id, avatar_url.as_deref()).await?;

        // Update in-memory cache
        if let Some(mut entry) = self.metadata.get_mut(id) {
            entry.avatar_url = avatar_url;
        }

        Ok(())
    }

    /// Update a streamer with new metadata.
    ///
    /// Persists to database first, then updates in-memory cache.
    /// This method allows updating all mutable fields of a streamer.
    pub async fn update_streamer(&self, metadata: StreamerMetadata) -> Result<()> {
        debug!("Updating streamer: {}", metadata.id);

        // Check if streamer exists
        if !self.metadata.contains_key(&metadata.id) {
            return Err(crate::Error::not_found("Streamer", &metadata.id));
        }

        // Convert to DB model and persist
        let db_model = self.metadata_to_db_model(&metadata);
        self.repo.update_streamer(&db_model).await?;

        // Update in-memory cache
        if let Some(old) = self.metadata.get(&metadata.id) {
            let old_url = old.url.to_lowercase();
            let new_url = metadata.url.to_lowercase();
            if old_url != new_url {
                self.url_index.remove(&old_url);
            }
            self.url_index.insert(new_url, metadata.id.clone());
        }
        self.metadata.insert(metadata.id.clone(), metadata.clone());

        // Broadcast event
        self.broadcaster
            .publish(ConfigUpdateEvent::StreamerMetadataUpdated {
                streamer_id: metadata.id,
            });

        Ok(())
    }

    /// Partially update a streamer.
    ///
    /// Only updates the fields that are provided (Some values).
    /// Persists to database first, then updates in-memory cache.
    ///
    /// # Events
    /// Always emits `ConfigUpdateEvent::StreamerMetadataUpdated`, including when the update changes
    /// the streamer's state (e.g., user disables a streamer). `StreamerStateSyncedFromDb` is reserved
    /// for transactional DB sync via `reload_from_repo()`.
    pub async fn partial_update_streamer(
        &self,
        params: StreamerUpdateParams,
    ) -> Result<StreamerMetadata> {
        let StreamerUpdateParams {
            id,
            name,
            url,
            template_config_id,
            priority,
            state,
            streamer_specific_config,
        } = params;
        debug!("Partially updating streamer: {}", id);

        // Get current metadata
        let mut metadata = self
            .metadata
            .get(&id)
            .map(|entry| entry.clone())
            .ok_or_else(|| crate::Error::not_found("Streamer", id.clone()))?;

        // Apply updates
        if let Some(new_name) = name {
            metadata.name = new_name;
        }
        if let Some(new_url) = url {
            let old_url = metadata.url.to_lowercase();
            let new_url_lower = new_url.to_lowercase();
            if old_url != new_url_lower {
                self.url_index.remove(&old_url);
                self.url_index.insert(new_url_lower, id.clone());
            }
            metadata.url = new_url;
        }
        if let Some(new_template) = template_config_id {
            metadata.template_config_id = new_template;
        }
        if let Some(new_priority) = priority {
            metadata.priority = new_priority;
        }
        if let Some(new_state) = state {
            metadata.state = new_state;
        }
        if let Some(new_config) = streamer_specific_config {
            metadata.streamer_specific_config = new_config;
        }

        // Convert to DB model and persist
        let db_model = self.metadata_to_db_model(&metadata);
        self.repo.update_streamer(&db_model).await?;

        // Update in-memory cache
        self.metadata.insert(id.to_string(), metadata.clone());

        // Broadcast event
        self.broadcaster
            .publish(ConfigUpdateEvent::StreamerMetadataUpdated {
                streamer_id: id.to_string(),
            });

        Ok(metadata)
    }

    /// Delete a streamer.
    ///
    /// Removes from database first, then from in-memory cache.
    /// Broadcasts a StreamerDeleted event to trigger cleanup of active resources.
    pub async fn delete_streamer(&self, id: &str) -> Result<()> {
        debug!("Deleting streamer: {}", id);

        // Remove from database
        self.repo.delete_streamer(id).await?;

        // Remove from in-memory cache
        if let Some((_, entry)) = self.metadata.remove(id) {
            self.url_index.remove(&entry.url.to_lowercase());
        }

        // Broadcast deletion event to trigger cleanup of active resources
        self.broadcaster
            .publish(ConfigUpdateEvent::StreamerDeleted {
                streamer_id: id.to_string(),
            });

        Ok(())
    }

    // ========== Query Operations (From Memory) ==========

    /// Get streamer metadata by ID.
    pub fn get_streamer(&self, id: &str) -> Option<StreamerMetadata> {
        self.metadata.get(id).map(|entry| entry.clone())
    }

    /// Get streamer metadata by URL (case-insensitive).
    pub fn get_streamer_by_url(&self, url: &str) -> Option<StreamerMetadata> {
        let url_lower = url.to_lowercase();
        let id = self.url_index.get(&url_lower)?;
        self.get_streamer(id.value())
    }

    /// Get all streamers.
    pub fn get_all(&self) -> Vec<StreamerMetadata> {
        self.metadata
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get all active streamers.
    ///
    /// Returns streamers in active states (NotLive, Live, OutOfSchedule, InspectingLive).
    pub fn get_all_active(&self) -> Vec<StreamerMetadata> {
        self.metadata
            .iter()
            .filter(|entry| entry.is_active())
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get streamers by priority level.
    ///
    /// Returns streamers sorted by priority (High first).
    pub fn get_by_priority(&self, priority: Priority) -> Vec<StreamerMetadata> {
        self.metadata
            .iter()
            .filter(|entry| entry.priority == priority)
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get streamers by platform.
    pub fn get_by_platform(&self, platform_id: &str) -> Vec<StreamerMetadata> {
        self.metadata
            .iter()
            .filter(|entry| entry.platform_config_id == platform_id)
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get streamers by template.
    pub fn get_by_template(&self, template_id: &str) -> Vec<StreamerMetadata> {
        self.metadata
            .iter()
            .filter(|entry| entry.template_config_id.as_deref() == Some(template_id))
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get streamers ready for live checking.
    ///
    /// Returns active streamers that are not currently disabled.
    pub fn get_ready_for_check(&self) -> Vec<StreamerMetadata> {
        self.metadata
            .iter()
            .filter(|entry| entry.is_ready_for_check())
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get streamers sorted by priority (High first, then Normal, then Low).
    pub fn get_all_sorted_by_priority(&self) -> Vec<StreamerMetadata> {
        let mut streamers: Vec<_> = self.get_all();
        streamers.sort_by(|a, b| b.priority.cmp(&a.priority));
        streamers
    }

    // ========== Error Handling with Exponential Backoff ==========

    /// Record an error for a streamer.
    ///
    /// Increments consecutive_error_count and applies exponential backoff
    /// if the threshold is reached.
    pub async fn record_error(&self, id: &str, error: &str) -> Result<()> {
        warn!("Recording error for streamer {}: {}", id, error);

        let (new_count, disabled_until) = {
            let entry = self.metadata.get(id);
            let current_count = entry.map(|e| e.consecutive_error_count).unwrap_or(0);
            let new_count = current_count + 1;

            let disabled_until = if new_count >= self.error_threshold {
                Some(self.calculate_backoff(new_count))
            } else {
                None
            };

            (new_count, disabled_until)
        };

        // Persist to database
        self.repo
            .record_streamer_error(id, new_count, disabled_until, Some(error))
            .await?;

        // Update in-memory cache
        if let Some(mut entry) = self.metadata.get_mut(id) {
            entry.consecutive_error_count = new_count;
            entry.disabled_until = disabled_until;
            entry.last_error = Some(error.to_string());
            // Sync state with DB: TemporalDisabled when backoff is applied
            if disabled_until.is_some() {
                entry.state = StreamerState::TemporalDisabled;
            }
        }

        if let Some(until) = disabled_until {
            info!(
                "Streamer {} disabled until {} due to {} consecutive errors",
                id, until, new_count
            );
        }

        Ok(())
    }

    /// Compute the disabled-until timestamp for a given consecutive error count.
    ///
    /// Returns `Some(timestamp)` when the error threshold is reached and the streamer
    /// should enter exponential backoff, otherwise `None`.
    pub fn disabled_until_for_error_count(&self, error_count: i32) -> Option<DateTime<Utc>> {
        if error_count >= self.error_threshold {
            Some(self.calculate_backoff(error_count))
        } else {
            None
        }
    }

    /// Record a successful operation for a streamer.
    ///
    /// Resets consecutive_error_count and clears disabled_until.
    /// Updates last_live_time if the streamer is going live.
    pub async fn record_success(&self, id: &str, is_going_live: bool) -> Result<()> {
        debug!("Recording success for streamer {}", id);

        let last_live_time = if is_going_live {
            Some(Utc::now())
        } else {
            None
        };

        // Persist to database
        self.repo
            .record_streamer_success(id, last_live_time)
            .await?;

        // Update in-memory cache
        if let Some(mut entry) = self.metadata.get_mut(id) {
            entry.consecutive_error_count = 0;
            entry.disabled_until = None;
            entry.last_error = None;
            if let Some(time) = last_live_time {
                entry.last_live_time = Some(time);
            }
        }

        Ok(())
    }

    /// Check if a streamer is currently disabled.
    pub fn is_disabled(&self, id: &str) -> bool {
        self.metadata
            .get(id)
            .map(|entry| entry.is_disabled())
            .unwrap_or(false)
    }

    // ========== Statistics ==========

    /// Get the total number of streamers.
    pub fn count(&self) -> usize {
        self.metadata.len()
    }

    /// Get the number of active streamers.
    pub fn active_count(&self) -> usize {
        self.metadata.iter().filter(|e| e.is_active()).count()
    }

    /// Get the number of disabled streamers.
    pub fn disabled_count(&self) -> usize {
        self.metadata.iter().filter(|e| e.is_disabled()).count()
    }

    /// Get the number of live streamers.
    pub fn live_count(&self) -> usize {
        self.metadata
            .iter()
            .filter(|e| e.state == StreamerState::Live)
            .count()
    }

    /// Get a reference to the underlying metadata store.
    ///
    /// This is useful for actors that need direct read access to streamer metadata
    /// without going through the manager's methods. The returned Arc can be cloned
    /// and shared with actors for efficient metadata lookups.
    pub fn metadata_store(&self) -> Arc<DashMap<String, StreamerMetadata>> {
        self.metadata.clone()
    }

    // ========== URL Uniqueness Checks ==========

    /// Check if a URL already exists in the system.
    ///
    /// Performs case-insensitive comparison.
    pub fn url_exists(&self, url: &str) -> bool {
        let url_lower = url.to_lowercase();
        self.url_index.contains_key(&url_lower)
    }

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

    /// Calculate backoff duration based on error count.
    fn calculate_backoff(&self, error_count: i32) -> DateTime<Utc> {
        let exponent = (error_count - self.error_threshold).max(0) as u32;
        let backoff_secs = (BASE_BACKOFF_SECS * 2u64.pow(exponent)).min(MAX_BACKOFF_SECS);
        Utc::now() + chrono::Duration::seconds(backoff_secs as i64)
    }

    /// Convert metadata to database model.
    fn metadata_to_db_model(
        &self,
        metadata: &StreamerMetadata,
    ) -> crate::database::models::StreamerDbModel {
        crate::database::models::StreamerDbModel {
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
            disabled_until: metadata.disabled_until.map(|dt| dt.timestamp_millis()),
            last_live_time: metadata.last_live_time.map(|dt| dt.timestamp_millis()),
            streamer_specific_config: metadata.streamer_specific_config.clone(),
            created_at: metadata.created_at.timestamp_millis(),
            updated_at: metadata.updated_at.timestamp_millis(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::models::StreamerDbModel;
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// Mock streamer repository for testing.
    struct MockStreamerRepository {
        streamers: Mutex<Vec<StreamerDbModel>>,
    }

    impl MockStreamerRepository {
        fn new() -> Self {
            Self {
                streamers: Mutex::new(Vec::new()),
            }
        }

        fn with_streamers(streamers: Vec<StreamerDbModel>) -> Self {
            Self {
                streamers: Mutex::new(streamers),
            }
        }
    }

    #[async_trait]
    impl StreamerRepository for MockStreamerRepository {
        async fn list_all_streamers(&self) -> Result<Vec<StreamerDbModel>> {
            Ok(self.streamers.lock().unwrap().clone())
        }

        async fn get_streamer(&self, id: &str) -> Result<StreamerDbModel> {
            self.streamers
                .lock()
                .unwrap()
                .iter()
                .find(|s| s.id == id)
                .cloned()
                .ok_or_else(|| crate::Error::not_found("Streamer", id))
        }

        async fn get_streamer_by_url(&self, url: &str) -> Result<StreamerDbModel> {
            self.streamers
                .lock()
                .unwrap()
                .iter()
                .find(|s| s.url == url)
                .cloned()
                .ok_or_else(|| crate::Error::not_found("Streamer", url))
        }

        async fn list_streamers(&self) -> Result<Vec<StreamerDbModel>> {
            Ok(self.streamers.lock().unwrap().clone())
        }

        async fn list_streamers_by_state(&self, state: &str) -> Result<Vec<StreamerDbModel>> {
            Ok(self
                .streamers
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.state == state)
                .cloned()
                .collect())
        }

        async fn list_streamers_by_priority(&self, priority: &str) -> Result<Vec<StreamerDbModel>> {
            Ok(self
                .streamers
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.priority == priority)
                .cloned()
                .collect())
        }

        async fn list_active_streamers(&self) -> Result<Vec<StreamerDbModel>> {
            Ok(self.streamers.lock().unwrap().clone())
        }

        async fn create_streamer(&self, streamer: &StreamerDbModel) -> Result<()> {
            self.streamers.lock().unwrap().push(streamer.clone());
            Ok(())
        }

        async fn update_streamer(&self, streamer: &StreamerDbModel) -> Result<()> {
            let mut streamers = self.streamers.lock().unwrap();
            if let Some(s) = streamers.iter_mut().find(|s| s.id == streamer.id) {
                s.name = streamer.name.clone();
                s.url = streamer.url.clone();
                s.platform_config_id = streamer.platform_config_id.clone();
                s.template_config_id = streamer.template_config_id.clone();
                s.state = streamer.state.clone();
                s.priority = streamer.priority.clone();
                s.last_live_time = streamer.last_live_time;
                s.consecutive_error_count = streamer.consecutive_error_count;
                s.disabled_until = streamer.disabled_until;
                s.last_error = streamer.last_error.clone();
            }
            Ok(())
        }

        async fn delete_streamer(&self, id: &str) -> Result<()> {
            self.streamers.lock().unwrap().retain(|s| s.id != id);
            Ok(())
        }

        async fn update_streamer_state(&self, _id: &str, _state: &str) -> Result<()> {
            Ok(())
        }

        async fn update_streamer_priority(&self, _id: &str, _priority: &str) -> Result<()> {
            Ok(())
        }

        async fn increment_error_count(&self, _id: &str) -> Result<i32> {
            Ok(1)
        }

        async fn reset_error_count(&self, _id: &str) -> Result<()> {
            Ok(())
        }

        async fn set_disabled_until(&self, _id: &str, _until: Option<i64>) -> Result<()> {
            Ok(())
        }

        async fn update_last_live_time(&self, id: &str, time: i64) -> Result<()> {
            let mut streamers = self.streamers.lock().unwrap();
            if let Some(s) = streamers.iter_mut().find(|s| s.id == id) {
                s.last_live_time = Some(time);
                Ok(())
            } else {
                Err(crate::Error::not_found("Streamer", id))
            }
        }

        async fn update_avatar(&self, id: &str, avatar_url: Option<&str>) -> Result<()> {
            let mut streamers = self.streamers.lock().unwrap();
            if let Some(s) = streamers.iter_mut().find(|s| s.id == id) {
                s.avatar = avatar_url.map(|s| s.to_string());
                Ok(())
            } else {
                Err(crate::Error::not_found("Streamer", id))
            }
        }

        async fn clear_streamer_error_state(&self, _id: &str) -> Result<()> {
            Ok(())
        }

        async fn record_streamer_error(
            &self,
            _id: &str,
            _error_count: i32,
            _disabled_until: Option<DateTime<Utc>>,
            _error: Option<&str>,
        ) -> Result<()> {
            Ok(())
        }

        async fn record_streamer_success(
            &self,
            _id: &str,
            _last_live_time: Option<DateTime<Utc>>,
        ) -> Result<()> {
            Ok(())
        }

        async fn list_streamers_by_platform(
            &self,
            platform_id: &str,
        ) -> Result<Vec<StreamerDbModel>> {
            Ok(self
                .streamers
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.platform_config_id == platform_id)
                .cloned()
                .collect())
        }

        async fn list_streamers_by_template(
            &self,
            template_id: &str,
        ) -> Result<Vec<StreamerDbModel>> {
            Ok(self
                .streamers
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.template_config_id.as_deref() == Some(template_id))
                .cloned()
                .collect())
        }
    }

    fn create_test_manager() -> StreamerManager<MockStreamerRepository> {
        let repo = Arc::new(MockStreamerRepository::new());
        let broadcaster = ConfigEventBroadcaster::new();
        StreamerManager::new(repo, broadcaster)
    }

    fn create_test_streamer(id: &str, url: &str) -> StreamerMetadata {
        StreamerMetadata {
            id: id.to_string(),
            name: format!("Streamer {}", id),
            url: url.to_string(),
            platform_config_id: "test".to_string(),
            template_config_id: None,
            state: StreamerState::NotLive,
            priority: Priority::Normal,
            avatar_url: None,
            consecutive_error_count: 0,
            disabled_until: None,
            last_live_time: None,
            last_error: None,
            streamer_specific_config: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_get_streamer_by_url_updates_on_change() {
        let manager = create_test_manager();

        let mut metadata = create_test_streamer("s1", "https://example.com/one");
        manager.create_streamer(metadata.clone()).await.unwrap();

        let by_url = manager.get_streamer_by_url("https://example.com/one");
        assert!(by_url.is_some());

        metadata.url = "https://example.com/two".to_string();
        manager.update_streamer(metadata.clone()).await.unwrap();

        assert!(
            manager
                .get_streamer_by_url("https://example.com/one")
                .is_none()
        );
        assert!(
            manager
                .get_streamer_by_url("https://example.com/two")
                .is_some()
        );
    }

    #[tokio::test]
    async fn test_get_streamer_by_url_removed_on_delete() {
        let manager = create_test_manager();

        let metadata = create_test_streamer("s1", "https://example.com/one");
        manager.create_streamer(metadata).await.unwrap();

        assert!(
            manager
                .get_streamer_by_url("https://example.com/one")
                .is_some()
        );

        manager.delete_streamer("s1").await.unwrap();
        assert!(
            manager
                .get_streamer_by_url("https://example.com/one")
                .is_none()
        );
    }
    fn create_test_db_model(id: &str, platform: &str) -> StreamerDbModel {
        StreamerDbModel {
            id: id.to_string(),
            name: format!("Streamer {}", id),
            url: format!("https://example.com/{}", id),
            platform_config_id: platform.to_string(),
            template_config_id: None,
            state: "NOT_LIVE".to_string(),
            priority: "NORMAL".to_string(),
            avatar: None,
            consecutive_error_count: Some(0),
            last_error: None,
            disabled_until: None,
            last_live_time: None,
            streamer_specific_config: None,
            created_at: Utc::now().timestamp_millis(),
            updated_at: Utc::now().timestamp_millis(),
        }
    }

    #[tokio::test]
    async fn test_hydrate() {
        let repo = MockStreamerRepository::with_streamers(vec![
            create_test_db_model("s1", "twitch"),
            create_test_db_model("s2", "youtube"),
        ]);
        let broadcaster = ConfigEventBroadcaster::new();
        let manager = StreamerManager::new(Arc::new(repo), broadcaster);

        let count = manager.hydrate().await.unwrap();
        assert_eq!(count, 2);
        assert_eq!(manager.count(), 2);
    }

    #[tokio::test]
    async fn test_create_streamer() {
        let repo = MockStreamerRepository::new();
        let broadcaster = ConfigEventBroadcaster::new();
        let manager = StreamerManager::new(Arc::new(repo), broadcaster);

        let metadata = StreamerMetadata {
            id: "new-streamer".to_string(),
            name: "New Streamer".to_string(),
            url: "https://twitch.tv/new".to_string(),
            platform_config_id: "twitch".to_string(),
            template_config_id: None,
            state: StreamerState::NotLive,
            priority: Priority::Normal,
            avatar_url: None,
            consecutive_error_count: 0,
            last_error: None,
            disabled_until: None,
            last_live_time: None,
            streamer_specific_config: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        manager.create_streamer(metadata.clone()).await.unwrap();

        let retrieved = manager.get_streamer("new-streamer");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, "New Streamer");
    }

    #[tokio::test]
    async fn test_update_state() {
        let repo =
            MockStreamerRepository::with_streamers(vec![create_test_db_model("s1", "twitch")]);
        let broadcaster = ConfigEventBroadcaster::new();
        let manager = StreamerManager::new(Arc::new(repo), broadcaster);
        manager.hydrate().await.unwrap();

        manager
            .update_state("s1", StreamerState::Live)
            .await
            .unwrap();

        let metadata = manager.get_streamer("s1").unwrap();
        assert_eq!(metadata.state, StreamerState::Live);
    }

    #[tokio::test]
    async fn test_get_by_platform() {
        let repo = MockStreamerRepository::with_streamers(vec![
            create_test_db_model("s1", "twitch"),
            create_test_db_model("s2", "twitch"),
            create_test_db_model("s3", "youtube"),
        ]);
        let broadcaster = ConfigEventBroadcaster::new();
        let manager = StreamerManager::new(Arc::new(repo), broadcaster);
        manager.hydrate().await.unwrap();

        let twitch_streamers = manager.get_by_platform("twitch");
        assert_eq!(twitch_streamers.len(), 2);

        let youtube_streamers = manager.get_by_platform("youtube");
        assert_eq!(youtube_streamers.len(), 1);
    }

    #[tokio::test]
    async fn test_record_error_with_backoff() {
        let repo =
            MockStreamerRepository::with_streamers(vec![create_test_db_model("s1", "twitch")]);
        let broadcaster = ConfigEventBroadcaster::new();
        let manager = StreamerManager::with_error_threshold(Arc::new(repo), broadcaster, 2);
        manager.hydrate().await.unwrap();

        // First error - no backoff
        manager.record_error("s1", "Error 1").await.unwrap();
        let metadata = manager.get_streamer("s1").unwrap();
        assert_eq!(metadata.consecutive_error_count, 1);
        assert!(metadata.disabled_until.is_none());

        // Second error - triggers backoff
        manager.record_error("s1", "Error 2").await.unwrap();
        let metadata = manager.get_streamer("s1").unwrap();
        assert_eq!(metadata.consecutive_error_count, 2);
        assert!(metadata.disabled_until.is_some());
        assert!(metadata.is_disabled());
    }

    #[tokio::test]
    async fn test_record_success_clears_errors() {
        let repo =
            MockStreamerRepository::with_streamers(vec![create_test_db_model("s1", "twitch")]);
        let broadcaster = ConfigEventBroadcaster::new();
        let manager = StreamerManager::with_error_threshold(Arc::new(repo), broadcaster, 1);
        manager.hydrate().await.unwrap();

        // Record error to trigger backoff
        manager.record_error("s1", "Error").await.unwrap();
        assert!(manager.is_disabled("s1"));

        // Record success
        manager.record_success("s1", false).await.unwrap();
        let metadata = manager.get_streamer("s1").unwrap();
        assert_eq!(metadata.consecutive_error_count, 0);
        assert!(metadata.disabled_until.is_none());
        assert!(!manager.is_disabled("s1"));
    }

    #[tokio::test]
    async fn test_delete_streamer() {
        let repo =
            MockStreamerRepository::with_streamers(vec![create_test_db_model("s1", "twitch")]);
        let broadcaster = ConfigEventBroadcaster::new();
        let manager = StreamerManager::new(Arc::new(repo), broadcaster);
        manager.hydrate().await.unwrap();

        assert!(manager.get_streamer("s1").is_some());

        manager.delete_streamer("s1").await.unwrap();

        assert!(manager.get_streamer("s1").is_none());
    }

    #[tokio::test]
    async fn test_update_streamer() {
        let repo =
            MockStreamerRepository::with_streamers(vec![create_test_db_model("s1", "twitch")]);
        let broadcaster = ConfigEventBroadcaster::new();
        let manager = StreamerManager::new(Arc::new(repo), broadcaster);
        manager.hydrate().await.unwrap();

        // Get current metadata and modify it
        let mut metadata = manager.get_streamer("s1").unwrap();
        metadata.name = "Updated Name".to_string();
        metadata.priority = Priority::High;
        metadata.template_config_id = Some("template-1".to_string());

        // Update the streamer
        manager.update_streamer(metadata).await.unwrap();

        // Verify the update
        let updated = manager.get_streamer("s1").unwrap();
        assert_eq!(updated.name, "Updated Name");
        assert_eq!(updated.priority, Priority::High);
        assert_eq!(updated.template_config_id, Some("template-1".to_string()));
    }

    #[tokio::test]
    async fn test_update_streamer_not_found() {
        let repo = MockStreamerRepository::new();
        let broadcaster = ConfigEventBroadcaster::new();
        let manager = StreamerManager::new(Arc::new(repo), broadcaster);

        let metadata = StreamerMetadata {
            id: "nonexistent".to_string(),
            name: "Test".to_string(),
            url: "https://example.com".to_string(),
            platform_config_id: "twitch".to_string(),
            template_config_id: None,
            state: StreamerState::NotLive,
            priority: Priority::Normal,
            avatar_url: None,
            consecutive_error_count: 0,
            disabled_until: None,
            last_error: None,
            last_live_time: None,
            streamer_specific_config: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let result = manager.update_streamer(metadata).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_partial_update_streamer() {
        let repo =
            MockStreamerRepository::with_streamers(vec![create_test_db_model("s1", "twitch")]);
        let broadcaster = ConfigEventBroadcaster::new();
        let manager = StreamerManager::new(Arc::new(repo), broadcaster);
        manager.hydrate().await.unwrap();

        // Partial update - only name and priority
        let updated = manager
            .partial_update_streamer(StreamerUpdateParams {
                id: "s1".to_string(),
                name: Some("New Name".to_string()),
                url: None,                // Don't change URL
                template_config_id: None, // Don't change template
                priority: Some(Priority::High),
                state: None, // Don't change state
                streamer_specific_config: None,
            })
            .await
            .unwrap();

        assert_eq!(updated.name, "New Name");
        assert_eq!(updated.priority, Priority::High);
        // URL should remain unchanged
        assert_eq!(updated.url, "https://example.com/s1");
    }

    #[tokio::test]
    async fn test_partial_update_template_to_none() {
        let mut db_model = create_test_db_model("s1", "twitch");
        db_model.template_config_id = Some("old-template".to_string());

        let repo = MockStreamerRepository::with_streamers(vec![db_model]);
        let broadcaster = ConfigEventBroadcaster::new();
        let manager = StreamerManager::new(Arc::new(repo), broadcaster);
        manager.hydrate().await.unwrap();

        // Verify initial template
        let initial = manager.get_streamer("s1").unwrap();
        assert_eq!(initial.template_config_id, Some("old-template".to_string()));

        // Update template to None
        let updated = manager
            .partial_update_streamer(StreamerUpdateParams {
                id: "s1".to_string(),
                name: None,
                url: None,                      // Don't change URL
                template_config_id: Some(None), // Set template to None
                priority: None,
                state: None,
                streamer_specific_config: None,
            })
            .await
            .unwrap();

        assert_eq!(updated.template_config_id, None);
    }
}
