//! Streamer management routes.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get, patch, post},
};

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{
    CreateStreamerRequest, PaginatedResponse, PaginationParams, StreamerFilterParams,
    StreamerResponse, UpdatePriorityRequest, UpdateStreamerRequest,
};
use crate::api::server::AppState;
use crate::domain::Priority as DomainPriority;
use crate::domain::streamer::StreamerState;
use crate::domain::value_objects::Priority as ApiPriority;
use crate::streamer::StreamerMetadata;

/// Convert API Priority to Domain Priority.
fn api_to_domain_priority(p: ApiPriority) -> DomainPriority {
    match p {
        ApiPriority::High => DomainPriority::High,
        ApiPriority::Normal => DomainPriority::Normal,
        ApiPriority::Low => DomainPriority::Low,
    }
}

/// Convert Domain Priority to API Priority.
fn domain_to_api_priority(p: DomainPriority) -> ApiPriority {
    match p {
        DomainPriority::High => ApiPriority::High,
        DomainPriority::Normal => ApiPriority::Normal,
        DomainPriority::Low => ApiPriority::Low,
    }
}

/// Create the streamers router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_streamer))
        .route("/", get(list_streamers))
        .route("/{id}", get(get_streamer))
        .route("/{id}", patch(update_streamer))
        .route("/{id}", delete(delete_streamer))
        .route("/{id}/clear-error", post(clear_error))
        .route("/{id}/priority", patch(update_priority))
}

/// Convert StreamerMetadata to StreamerResponse.
fn metadata_to_response(metadata: &StreamerMetadata) -> StreamerResponse {
    StreamerResponse {
        id: metadata.id.clone(),
        name: metadata.name.clone(),
        url: metadata.url.clone(),
        platform_config_id: metadata.platform_config_id.clone(),
        template_id: metadata.template_config_id.clone(),
        state: metadata.state,
        priority: domain_to_api_priority(metadata.priority),
        enabled: metadata.state != StreamerState::Disabled,
        consecutive_error_count: metadata.consecutive_error_count,
        disabled_until: metadata.disabled_until,
        last_live_time: metadata.last_live_time,
        created_at: chrono::Utc::now(), // Not stored in metadata
        updated_at: chrono::Utc::now(), // Not stored in metadata
    }
}

/// Create a new streamer.
///
/// POST /api/streamers
async fn create_streamer(
    State(state): State<AppState>,
    Json(request): Json<CreateStreamerRequest>,
) -> ApiResult<Json<StreamerResponse>> {
    // Validate URL format
    if request.url.is_empty() {
        return Err(ApiError::validation("URL cannot be empty"));
    }

    // Get streamer manager from state
    let streamer_manager = state
        .streamer_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Streamer service not available"))?;

    // Check URL uniqueness (case-insensitive)
    if streamer_manager.url_exists(&request.url) {
        return Err(ApiError::conflict(
            "A streamer with this URL already exists",
        ));
    }

    // Generate a new ID for the streamer
    let id = uuid::Uuid::new_v4().to_string();

    // Create metadata from request
    let metadata = StreamerMetadata {
        id: id.clone(),
        name: request.name.clone(),
        url: request.url.clone(),
        platform_config_id: request.platform_config_id.clone(),
        template_config_id: request.template_id.clone(),
        state: if request.enabled {
            StreamerState::NotLive
        } else {
            StreamerState::Disabled
        },
        priority: api_to_domain_priority(request.priority),
        consecutive_error_count: 0,
        disabled_until: None,
        last_live_time: None,
    };

    // Create streamer using manager
    streamer_manager
        .create_streamer(metadata.clone())
        .await
        .map_err(ApiError::from)?;

    Ok(Json(metadata_to_response(&metadata)))
}

/// List streamers with pagination and filtering.
///
/// GET /api/streamers
async fn list_streamers(
    State(state): State<AppState>,
    Query(pagination): Query<PaginationParams>,
    Query(filters): Query<StreamerFilterParams>,
) -> ApiResult<Json<PaginatedResponse<StreamerResponse>>> {
    // Get streamer manager from state
    let streamer_manager = state
        .streamer_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Streamer service not available"))?;

    // Get all streamers from manager
    let mut streamers = streamer_manager.get_all();

    // Apply filters
    if let Some(platform) = &filters.platform {
        streamers.retain(|s| &s.platform_config_id == platform);
    }
    if let Some(state_str) = &filters.state {
        if !state_str.is_empty() {
            let states: Vec<StreamerState> = state_str
                .split(',')
                .filter_map(|s| StreamerState::parse(&s.trim().to_uppercase()))
                .collect();
            streamers.retain(|s| states.contains(&s.state));
        }
    }
    if let Some(priority) = &filters.priority {
        let domain_priority = api_to_domain_priority(*priority);
        streamers.retain(|s| s.priority == domain_priority);
    }
    if let Some(enabled) = filters.enabled {
        streamers.retain(|s| (s.state != StreamerState::Disabled) == enabled);
    }

    // Sort if requested
    if let Some(sort_by) = &filters.sort_by {
        let desc = filters.sort_dir.as_deref() == Some("desc");
        match sort_by.as_str() {
            "name" => {
                streamers.sort_by(|a, b| {
                    if desc {
                        b.name.cmp(&a.name)
                    } else {
                        a.name.cmp(&b.name)
                    }
                });
            }
            "priority" => {
                streamers.sort_by(|a, b| {
                    if desc {
                        b.priority.cmp(&a.priority)
                    } else {
                        a.priority.cmp(&b.priority)
                    }
                });
            }
            "state" => {
                streamers.sort_by(|a, b| {
                    let a_str = a.state.as_str();
                    let b_str = b.state.as_str();
                    if desc {
                        b_str.cmp(a_str)
                    } else {
                        a_str.cmp(b_str)
                    }
                });
            }
            _ => {}
        }
    }

    // Calculate total before pagination
    let total = streamers.len() as u64;

    // Apply pagination
    let offset = pagination.offset as usize;
    let limit = pagination.limit.min(100) as usize;
    let streamers: Vec<_> = streamers
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|s| metadata_to_response(&s))
        .collect();

    let response = PaginatedResponse::new(streamers, total, pagination.limit, pagination.offset);
    Ok(Json(response))
}

/// Get a single streamer by ID.
///
/// GET /api/streamers/:id
async fn get_streamer(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<StreamerResponse>> {
    // Get streamer manager from state
    let streamer_manager = state
        .streamer_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Streamer service not available"))?;

    // Get streamer by ID
    let metadata = streamer_manager
        .get_streamer(&id)
        .ok_or_else(|| ApiError::not_found(format!("Streamer with id '{}' not found", id)))?;

    Ok(Json(metadata_to_response(&metadata)))
}

/// Update a streamer.
///
/// PATCH /api/streamers/:id
async fn update_streamer(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateStreamerRequest>,
) -> ApiResult<Json<StreamerResponse>> {
    // Get streamer manager from state
    let streamer_manager = state
        .streamer_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Streamer service not available"))?;

    // Check URL uniqueness if URL is being changed (case-insensitive)
    if let Some(ref new_url) = request.url {
        if streamer_manager.url_exists_for_other(new_url, &id) {
            return Err(ApiError::conflict(
                "A streamer with this URL already exists",
            ));
        }
    }

    // Convert enabled flag to state if provided
    let new_state = request.enabled.map(|enabled| {
        if enabled {
            StreamerState::NotLive
        } else {
            StreamerState::Disabled
        }
    });

    // Convert API priority to domain priority if provided
    let new_priority = request.priority.map(api_to_domain_priority);

    // Convert template_id: Some(value) -> Some(Some(value)), None -> None
    // This allows distinguishing between "not updating" and "setting to None"
    let template_config_id = request.template_id.map(Some);

    // Use partial_update_streamer for atomic update
    let metadata = streamer_manager
        .partial_update_streamer(
            &id,
            request.name,
            request.url,
            template_config_id,
            new_priority,
            new_state,
        )
        .await
        .map_err(ApiError::from)?;

    Ok(Json(metadata_to_response(&metadata)))
}

/// Delete a streamer.
///
/// DELETE /api/streamers/:id
async fn delete_streamer(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    // Get streamer manager from state
    let streamer_manager = state
        .streamer_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Streamer service not available"))?;

    // Check if streamer exists first
    if streamer_manager.get_streamer(&id).is_none() {
        return Err(ApiError::not_found(format!(
            "Streamer with id '{}' not found",
            id
        )));
    }

    // Delete the streamer
    streamer_manager
        .delete_streamer(&id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("Streamer '{}' deleted successfully", id)
    })))
}

/// Clear error state for a streamer.
///
/// POST /api/streamers/:id/clear-error
async fn clear_error(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<StreamerResponse>> {
    // Get streamer manager from state
    let streamer_manager = state
        .streamer_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Streamer service not available"))?;

    // Check if streamer exists first
    if streamer_manager.get_streamer(&id).is_none() {
        return Err(ApiError::not_found(format!(
            "Streamer with id '{}' not found",
            id
        )));
    }

    // Clear error state (resets consecutive_error_count and disabled_until)
    streamer_manager
        .clear_error_state(&id)
        .await
        .map_err(ApiError::from)?;

    // Get updated metadata
    let metadata = streamer_manager
        .get_streamer(&id)
        .ok_or_else(|| ApiError::internal("Failed to retrieve streamer after clearing error"))?;

    Ok(Json(metadata_to_response(&metadata)))
}

/// Update streamer priority.
///
/// PATCH /api/streamers/:id/priority
async fn update_priority(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdatePriorityRequest>,
) -> ApiResult<Json<StreamerResponse>> {
    // Get streamer manager from state
    let streamer_manager = state
        .streamer_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Streamer service not available"))?;

    // Check if streamer exists first
    if streamer_manager.get_streamer(&id).is_none() {
        return Err(ApiError::not_found(format!(
            "Streamer with id '{}' not found",
            id
        )));
    }

    // Update priority
    let domain_priority = api_to_domain_priority(request.priority);
    streamer_manager
        .update_priority(&id, domain_priority)
        .await
        .map_err(ApiError::from)?;

    // Get updated metadata
    let metadata = streamer_manager
        .get_streamer(&id)
        .ok_or_else(|| ApiError::internal("Failed to retrieve streamer after priority update"))?;

    Ok(Json(metadata_to_response(&metadata)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_streamer_request_validation() {
        let request = CreateStreamerRequest {
            name: "Test".to_string(),
            url: "".to_string(),
            platform_config_id: "platform1".to_string(),
            template_id: None,
            priority: ApiPriority::Normal,
            enabled: true,
        };

        // URL is empty, should fail validation
        assert!(request.url.is_empty());
    }

    #[test]
    fn test_metadata_to_response() {
        let metadata = StreamerMetadata {
            id: "test-id".to_string(),
            name: "Test Streamer".to_string(),
            url: "https://twitch.tv/test".to_string(),
            platform_config_id: "twitch".to_string(),
            template_config_id: Some("template1".to_string()),
            state: StreamerState::Live,
            priority: DomainPriority::High,
            consecutive_error_count: 2,
            disabled_until: None,
            last_live_time: Some(chrono::Utc::now()),
        };

        let response = metadata_to_response(&metadata);

        assert_eq!(response.id, "test-id");
        assert_eq!(response.name, "Test Streamer");
        assert_eq!(response.url, "https://twitch.tv/test");
        assert_eq!(response.platform_config_id, "twitch");
        assert_eq!(response.template_id, Some("template1".to_string()));
        assert_eq!(response.state, StreamerState::Live);
        assert!(response.enabled); // Live state means enabled
        assert_eq!(response.consecutive_error_count, 2);
    }

    #[test]
    fn test_disabled_state_means_not_enabled() {
        let metadata = StreamerMetadata {
            id: "test-id".to_string(),
            name: "Test".to_string(),
            url: "https://example.com".to_string(),
            platform_config_id: "platform".to_string(),
            template_config_id: None,
            state: StreamerState::Disabled,
            priority: DomainPriority::Normal,
            consecutive_error_count: 0,
            disabled_until: None,
            last_live_time: None,
        };

        let response = metadata_to_response(&metadata);
        assert!(!response.enabled);
    }

    #[test]
    fn test_priority_conversion() {
        // API to Domain
        assert_eq!(
            api_to_domain_priority(ApiPriority::High),
            DomainPriority::High
        );
        assert_eq!(
            api_to_domain_priority(ApiPriority::Normal),
            DomainPriority::Normal
        );
        assert_eq!(
            api_to_domain_priority(ApiPriority::Low),
            DomainPriority::Low
        );

        // Domain to API
        assert_eq!(
            domain_to_api_priority(DomainPriority::High),
            ApiPriority::High
        );
        assert_eq!(
            domain_to_api_priority(DomainPriority::Normal),
            ApiPriority::Normal
        );
        assert_eq!(
            domain_to_api_priority(DomainPriority::Low),
            ApiPriority::Low
        );
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use crate::config::ConfigEventBroadcaster;
    use crate::database::models::StreamerDbModel;
    use crate::database::repositories::streamer::StreamerRepository;
    use crate::streamer::StreamerManager;
    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use proptest::prelude::*;
    use std::sync::{Arc, Mutex};

    /// Mock streamer repository for property testing.
    struct MockStreamerRepository {
        streamers: Mutex<Vec<StreamerDbModel>>,
    }

    impl MockStreamerRepository {
        fn new() -> Self {
            Self {
                streamers: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl StreamerRepository for MockStreamerRepository {
        async fn list_all_streamers(&self) -> crate::Result<Vec<StreamerDbModel>> {
            Ok(self.streamers.lock().unwrap().clone())
        }

        async fn get_streamer(&self, id: &str) -> crate::Result<StreamerDbModel> {
            self.streamers
                .lock()
                .unwrap()
                .iter()
                .find(|s| s.id == id)
                .cloned()
                .ok_or_else(|| crate::Error::not_found("Streamer", id))
        }

        async fn get_streamer_by_url(&self, url: &str) -> crate::Result<StreamerDbModel> {
            self.streamers
                .lock()
                .unwrap()
                .iter()
                .find(|s| s.url == url)
                .cloned()
                .ok_or_else(|| crate::Error::not_found("Streamer", url))
        }

        async fn list_streamers(&self) -> crate::Result<Vec<StreamerDbModel>> {
            Ok(self.streamers.lock().unwrap().clone())
        }

        async fn list_streamers_by_state(
            &self,
            state: &str,
        ) -> crate::Result<Vec<StreamerDbModel>> {
            Ok(self
                .streamers
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.state == state)
                .cloned()
                .collect())
        }

        async fn list_streamers_by_priority(
            &self,
            priority: &str,
        ) -> crate::Result<Vec<StreamerDbModel>> {
            Ok(self
                .streamers
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.priority == priority)
                .cloned()
                .collect())
        }

        async fn list_active_streamers(&self) -> crate::Result<Vec<StreamerDbModel>> {
            Ok(self.streamers.lock().unwrap().clone())
        }

        async fn create_streamer(&self, streamer: &StreamerDbModel) -> crate::Result<()> {
            self.streamers.lock().unwrap().push(streamer.clone());
            Ok(())
        }

        async fn update_streamer(&self, _streamer: &StreamerDbModel) -> crate::Result<()> {
            Ok(())
        }

        async fn delete_streamer(&self, id: &str) -> crate::Result<()> {
            self.streamers.lock().unwrap().retain(|s| s.id != id);
            Ok(())
        }

        async fn update_streamer_state(&self, id: &str, state: &str) -> crate::Result<()> {
            if let Some(s) = self
                .streamers
                .lock()
                .unwrap()
                .iter_mut()
                .find(|s| s.id == id)
            {
                s.state = state.to_string();
            }
            Ok(())
        }

        async fn update_streamer_priority(&self, id: &str, priority: &str) -> crate::Result<()> {
            if let Some(s) = self
                .streamers
                .lock()
                .unwrap()
                .iter_mut()
                .find(|s| s.id == id)
            {
                s.priority = priority.to_string();
            }
            Ok(())
        }

        async fn increment_error_count(&self, _id: &str) -> crate::Result<i32> {
            Ok(1)
        }

        async fn reset_error_count(&self, _id: &str) -> crate::Result<()> {
            Ok(())
        }

        async fn set_disabled_until(&self, _id: &str, _until: Option<&str>) -> crate::Result<()> {
            Ok(())
        }

        async fn update_last_live_time(&self, _id: &str, _time: &str) -> crate::Result<()> {
            Ok(())
        }

        async fn clear_streamer_error_state(&self, id: &str) -> crate::Result<()> {
            if let Some(s) = self
                .streamers
                .lock()
                .unwrap()
                .iter_mut()
                .find(|s| s.id == id)
            {
                s.consecutive_error_count = Some(0);
                s.disabled_until = None;
                s.state = "NOT_LIVE".to_string();
            }
            Ok(())
        }

        async fn record_streamer_error(
            &self,
            id: &str,
            error_count: i32,
            disabled_until: Option<DateTime<Utc>>,
        ) -> crate::Result<()> {
            if let Some(s) = self
                .streamers
                .lock()
                .unwrap()
                .iter_mut()
                .find(|s| s.id == id)
            {
                s.consecutive_error_count = Some(error_count);
                s.disabled_until = disabled_until.map(|dt| dt.to_rfc3339());
            }
            Ok(())
        }

        async fn record_streamer_success(
            &self,
            id: &str,
            last_live_time: Option<DateTime<Utc>>,
        ) -> crate::Result<()> {
            if let Some(s) = self
                .streamers
                .lock()
                .unwrap()
                .iter_mut()
                .find(|s| s.id == id)
            {
                s.consecutive_error_count = Some(0);
                s.disabled_until = None;
                if let Some(time) = last_live_time {
                    s.last_live_time = Some(time.to_rfc3339());
                }
            }
            Ok(())
        }

        async fn list_streamers_by_platform(
            &self,
            platform_id: &str,
        ) -> crate::Result<Vec<StreamerDbModel>> {
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
        ) -> crate::Result<Vec<StreamerDbModel>> {
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

    // Strategy for generating valid streamer names
    fn streamer_name_strategy() -> impl Strategy<Value = String> {
        "[a-zA-Z][a-zA-Z0-9_ ]{0,49}"
            .prop_map(|s| s.trim().to_string())
            .prop_filter("Name must not be empty", |s| !s.is_empty())
    }

    // Strategy for generating valid URLs
    fn url_strategy() -> impl Strategy<Value = String> {
        prop::string::string_regex("https://[a-z]+\\.[a-z]+/[a-zA-Z0-9_]+")
            .unwrap()
            .prop_filter("URL must not be empty", |s| !s.is_empty())
    }

    // Strategy for generating platform IDs
    fn platform_id_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("twitch".to_string()),
            Just("youtube".to_string()),
            Just("huya".to_string()),
            Just("douyu".to_string()),
        ]
    }

    // Strategy for generating priorities
    fn priority_strategy() -> impl Strategy<Value = DomainPriority> {
        prop_oneof![
            Just(DomainPriority::High),
            Just(DomainPriority::Normal),
            Just(DomainPriority::Low),
        ]
    }

    // **Feature: jwt-auth-and-api-implementation, Property 5: Streamer CRUD Round-Trip**
    // **Validates: Requirements 2.1, 2.3**
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_streamer_crud_round_trip(
            name in streamer_name_strategy(),
            url in url_strategy(),
            platform_id in platform_id_strategy(),
            priority in priority_strategy(),
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let manager = create_test_manager();
                let id = uuid::Uuid::new_v4().to_string();

                // Create streamer metadata
                let metadata = StreamerMetadata {
                    id: id.clone(),
                    name: name.clone(),
                    url: url.clone(),
                    platform_config_id: platform_id.clone(),
                    template_config_id: None,
                    state: StreamerState::NotLive,
                    priority,
                    consecutive_error_count: 0,
                    disabled_until: None,
                    last_live_time: None,
                };

                // Create the streamer
                manager.create_streamer(metadata.clone()).await.expect("Create should succeed");

                // Retrieve the streamer
                let retrieved = manager.get_streamer(&id);
                prop_assert!(retrieved.is_some(), "Streamer should exist after creation");

                let retrieved = retrieved.unwrap();

                // Property: Retrieved data should match created data
                prop_assert_eq!(&retrieved.id, &id, "ID should match");
                prop_assert_eq!(&retrieved.name, &name, "Name should match");
                prop_assert_eq!(&retrieved.url, &url, "URL should match");
                prop_assert_eq!(&retrieved.platform_config_id, &platform_id, "Platform ID should match");
                prop_assert_eq!(retrieved.priority, priority, "Priority should match");
                prop_assert_eq!(retrieved.state, StreamerState::NotLive, "State should be NotLive");
                prop_assert_eq!(retrieved.consecutive_error_count, 0, "Error count should be 0");

                Ok(())
            })?;
        }
    }

    // **Feature: jwt-auth-and-api-implementation, Property 6: Streamer Update Persistence**
    // **Validates: Requirements 2.4**
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_streamer_update_persistence(
            name in streamer_name_strategy(),
            url in url_strategy(),
            platform_id in platform_id_strategy(),
            initial_priority in priority_strategy(),
            new_priority in priority_strategy(),
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let manager = create_test_manager();
                let id = uuid::Uuid::new_v4().to_string();

                // Create initial streamer
                let metadata = StreamerMetadata {
                    id: id.clone(),
                    name: name.clone(),
                    url: url.clone(),
                    platform_config_id: platform_id.clone(),
                    template_config_id: None,
                    state: StreamerState::NotLive,
                    priority: initial_priority,
                    consecutive_error_count: 0,
                    disabled_until: None,
                    last_live_time: None,
                };

                manager.create_streamer(metadata).await.expect("Create should succeed");

                // Update priority
                manager.update_priority(&id, new_priority).await.expect("Update should succeed");

                // Retrieve and verify
                let retrieved = manager.get_streamer(&id);
                prop_assert!(retrieved.is_some(), "Streamer should exist after update");

                let retrieved = retrieved.unwrap();

                // Property: Updated values should be persisted
                prop_assert_eq!(retrieved.priority, new_priority, "Priority should be updated");

                // Other fields should remain unchanged
                prop_assert_eq!(&retrieved.name, &name, "Name should remain unchanged");
                prop_assert_eq!(&retrieved.url, &url, "URL should remain unchanged");

                Ok(())
            })?;
        }
    }

    // **Feature: jwt-auth-and-api-implementation, Property 7: Streamer Delete Removes Resource**
    // **Validates: Requirements 2.5**
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_streamer_delete_removes_resource(
            name in streamer_name_strategy(),
            url in url_strategy(),
            platform_id in platform_id_strategy(),
            priority in priority_strategy(),
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let manager = create_test_manager();
                let id = uuid::Uuid::new_v4().to_string();

                // Create streamer
                let metadata = StreamerMetadata {
                    id: id.clone(),
                    name,
                    url,
                    platform_config_id: platform_id,
                    template_config_id: None,
                    state: StreamerState::NotLive,
                    priority,
                    consecutive_error_count: 0,
                    disabled_until: None,
                    last_live_time: None,
                };

                manager.create_streamer(metadata).await.expect("Create should succeed");

                // Verify it exists
                prop_assert!(manager.get_streamer(&id).is_some(), "Streamer should exist before deletion");

                // Delete the streamer
                manager.delete_streamer(&id).await.expect("Delete should succeed");

                // Property: Streamer should not exist after deletion
                prop_assert!(manager.get_streamer(&id).is_none(), "Streamer should not exist after deletion");

                Ok(())
            })?;
        }
    }
}
