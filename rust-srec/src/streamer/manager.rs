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
    /// Streamer repository for persistence.
    repo: Arc<R>,
    /// Event broadcaster for config updates.
    broadcaster: ConfigEventBroadcaster,
    /// Error threshold before backoff.
    error_threshold: i32,
}

impl<R> StreamerManager<R>
where
    R: StreamerRepository + Send + Sync,
{
    /// Create a new StreamerManager.
    pub fn new(repo: Arc<R>, broadcaster: ConfigEventBroadcaster) -> Self {
        Self {
            metadata: Arc::new(DashMap::new()),
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
    pub async fn hydrate(&self) -> Result<usize> {
        info!("Hydrating streamer metadata from database");

        let streamers = self.repo.list_all_streamers().await?;
        let count = streamers.len();

        for streamer in streamers {
            let metadata = StreamerMetadata::from_db_model(&streamer);
            self.metadata.insert(metadata.id.clone(), metadata);
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

        // Broadcast event
        self.broadcaster
            .publish(ConfigUpdateEvent::StreamerUpdated {
                streamer_id: metadata.id,
            });

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
            entry.state = StreamerState::NotLive;
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
        self.metadata.insert(metadata.id.clone(), metadata.clone());

        // Broadcast event
        self.broadcaster
            .publish(ConfigUpdateEvent::StreamerUpdated {
                streamer_id: metadata.id,
            });

        Ok(())
    }

    /// Partially update a streamer.
    ///
    /// Only updates the fields that are provided (Some values).
    /// Persists to database first, then updates in-memory cache.
    pub async fn partial_update_streamer(
        &self,
        id: &str,
        name: Option<String>,
        url: Option<String>,
        template_config_id: Option<Option<String>>,
        priority: Option<Priority>,
        state: Option<StreamerState>,
    ) -> Result<StreamerMetadata> {
        debug!("Partially updating streamer: {}", id);

        // Get current metadata
        let mut metadata = self
            .metadata
            .get(id)
            .map(|entry| entry.clone())
            .ok_or_else(|| crate::Error::not_found("Streamer", id))?;

        // Apply updates
        if let Some(new_name) = name {
            metadata.name = new_name;
        }
        if let Some(new_url) = url {
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

        // Convert to DB model and persist
        let db_model = self.metadata_to_db_model(&metadata);
        self.repo.update_streamer(&db_model).await?;

        // Update in-memory cache
        self.metadata.insert(id.to_string(), metadata.clone());

        // Broadcast event
        self.broadcaster
            .publish(ConfigUpdateEvent::StreamerUpdated {
                streamer_id: id.to_string(),
            });

        Ok(metadata)
    }

    /// Delete a streamer.
    ///
    /// Removes from database first, then from in-memory cache.
    pub async fn delete_streamer(&self, id: &str) -> Result<()> {
        debug!("Deleting streamer: {}", id);

        // Remove from database
        self.repo.delete_streamer(id).await?;

        // Remove from in-memory cache
        self.metadata.remove(id);

        Ok(())
    }

    // ========== Query Operations (From Memory) ==========

    /// Get streamer metadata by ID.
    pub fn get_streamer(&self, id: &str) -> Option<StreamerMetadata> {
        self.metadata.get(id).map(|entry| entry.clone())
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
            .record_streamer_error(id, new_count, disabled_until)
            .await?;

        // Update in-memory cache
        if let Some(mut entry) = self.metadata.get_mut(id) {
            entry.consecutive_error_count = new_count;
            entry.disabled_until = disabled_until;
            if disabled_until.is_some() {
                entry.state = StreamerState::Error;
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

    // ========== URL Uniqueness Checks ==========

    /// Check if a URL already exists in the system.
    ///
    /// Performs case-insensitive comparison.
    pub fn url_exists(&self, url: &str) -> bool {
        let url_lower = url.to_lowercase();
        self.metadata
            .iter()
            .any(|entry| entry.url.to_lowercase() == url_lower)
    }

    /// Check if a URL exists for any streamer other than the specified one.
    ///
    /// Used during updates to allow a streamer to keep its own URL.
    /// Performs case-insensitive comparison.
    pub fn url_exists_for_other(&self, url: &str, exclude_id: &str) -> bool {
        let url_lower = url.to_lowercase();
        self.metadata
            .iter()
            .any(|entry| entry.url.to_lowercase() == url_lower && entry.id != exclude_id)
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
            consecutive_error_count: Some(metadata.consecutive_error_count),
            disabled_until: metadata.disabled_until.map(|dt| dt.to_rfc3339()),
            last_live_time: metadata.last_live_time.map(|dt| dt.to_rfc3339()),
            // These fields are not in metadata, use defaults
            download_retry_policy: None,
            danmu_sampling_config: None,
            streamer_specific_config: None,
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
                s.last_live_time = streamer.last_live_time.clone();
                s.consecutive_error_count = streamer.consecutive_error_count;
                s.disabled_until = streamer.disabled_until.clone();
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

        async fn set_disabled_until(&self, _id: &str, _until: Option<&str>) -> Result<()> {
            Ok(())
        }

        async fn update_last_live_time(&self, _id: &str, _time: &str) -> Result<()> {
            Ok(())
        }

        async fn clear_streamer_error_state(&self, _id: &str) -> Result<()> {
            Ok(())
        }

        async fn record_streamer_error(
            &self,
            _id: &str,
            _error_count: i32,
            _disabled_until: Option<DateTime<Utc>>,
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

    fn create_test_db_model(id: &str, platform: &str) -> StreamerDbModel {
        StreamerDbModel {
            id: id.to_string(),
            name: format!("Streamer {}", id),
            url: format!("https://example.com/{}", id),
            platform_config_id: platform.to_string(),
            template_config_id: None,
            state: "NOT_LIVE".to_string(),
            priority: "NORMAL".to_string(),
            consecutive_error_count: Some(0),
            disabled_until: None,
            last_live_time: None,
            download_retry_policy: None,
            danmu_sampling_config: None,
            streamer_specific_config: None,
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
            consecutive_error_count: 0,
            disabled_until: None,
            last_live_time: None,
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
            consecutive_error_count: 0,
            disabled_until: None,
            last_live_time: None,
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
            .partial_update_streamer(
                "s1",
                Some("New Name".to_string()),
                None, // Don't change URL
                None, // Don't change template
                Some(Priority::High),
                None, // Don't change state
            )
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
            .partial_update_streamer(
                "s1",
                None,
                None, // Don't change URL
                Some(None), // Set template to None
                None,
                None,
            )
            .await
            .unwrap();

        assert_eq!(updated.template_config_id, None);
    }
}
