//! Configuration routes.

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, patch, put},
};
use tracing::debug;

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{GlobalConfigResponse, PlatformConfigResponse, UpdateGlobalConfigRequest};
use crate::api::server::AppState;
use crate::database::models::{GlobalConfigDbModel, PlatformConfigDbModel};

/// Helper trait for apply_updates macro to handle Option wrapping/unwrapping
trait ApplyUpdate<Source> {
    fn apply_update(&mut self, source: Source);
}

// For required fields (T), we update only if the source (Option<T>) is Some.
impl<T> ApplyUpdate<Option<T>> for T {
    fn apply_update(&mut self, source: Option<T>) {
        if let Some(v) = source {
            *self = v;
        }
    }
}

// For optional fields (Option<T>), we always update (overwriting with Some or None).
impl<T> ApplyUpdate<Option<T>> for Option<T> {
    fn apply_update(&mut self, source: Option<T>) {
        *self = source;
    }
}

/// Helper macro to apply optional updates from requests.
macro_rules! apply_updates {
    // Form 1: With tracker (for Global Config)
    ($target:ident, $source:ident, $tracker:ident; [
        $( $field:ident $(: $transform:expr)? ),* $(,)?
    ]) => {
        $(
            if let Some(val) = $source.$field {
                let result = apply_updates!(@val val, $($transform)?);
                $target.$field.apply_update(result);
                $tracker.push(stringify!($field));
            }
        )*
    };

    // Form 2: Without tracker (Unused but kept for completeness)
    ($target:ident, $source:ident; [
        $( $field:ident $(: $transform:expr)? ),* $(,)?
    ]) => {
        $(
            if let Some(val) = $source.$field {
                let result = apply_updates!(@val val, $($transform)?);
                $target.$field.apply_update(result);
            }
        )*
    };

    // Helper to handle optional transform
    (@val $val:ident, $transform:expr) => { ($transform)($val) };
    (@val $val:ident,) => { $val };
}

/// Create the config router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/global", get(get_global_config))
        .route("/global", patch(update_global_config))
        .route("/platforms", get(list_platform_configs))
        .route("/platforms/{id}", get(get_platform_config))
        .route("/platforms/{id}", put(replace_platform_config))
}

/// Map GlobalConfigDbModel to GlobalConfigResponse.
fn map_global_config_to_response(config: GlobalConfigDbModel) -> GlobalConfigResponse {
    GlobalConfigResponse {
        output_folder: config.output_folder,
        output_filename_template: config.output_filename_template,
        output_file_format: config.output_file_format,
        min_segment_size_bytes: config.min_segment_size_bytes as u64,
        max_download_duration_secs: config.max_download_duration_secs as u64,
        max_part_size_bytes: config.max_part_size_bytes as u64,
        max_concurrent_downloads: config.max_concurrent_downloads as u32,
        max_concurrent_uploads: config.max_concurrent_uploads as u32,
        max_concurrent_cpu_jobs: config.max_concurrent_cpu_jobs as u32,
        max_concurrent_io_jobs: config.max_concurrent_io_jobs as u32,
        streamer_check_delay_ms: config.streamer_check_delay_ms as u64,
        proxy_config: Some(config.proxy_config),
        offline_check_delay_ms: config.offline_check_delay_ms as u64,
        offline_check_count: config.offline_check_count as u32,
        default_download_engine: config.default_download_engine,
        record_danmu: config.record_danmu,
        job_history_retention_days: config.job_history_retention_days as u32,
        notification_event_log_retention_days: config.notification_event_log_retention_days as u32,
        session_gap_time_secs: config.session_gap_time_secs as u64,
        pipeline: config.pipeline,
        session_complete_pipeline: config.session_complete_pipeline,
        paired_segment_pipeline: config.paired_segment_pipeline,
        log_filter_directive: config.log_filter_directive,
        auto_thumbnail: config.auto_thumbnail,
    }
}

/// Map PlatformConfigDbModel to PlatformConfigResponse.
fn map_platform_config_to_response(config: PlatformConfigDbModel) -> PlatformConfigResponse {
    PlatformConfigResponse {
        id: config.id,
        name: config.platform_name,
        fetch_delay_ms: config.fetch_delay_ms.map(|v| v as u64),
        download_delay_ms: config.download_delay_ms.map(|v| v as u64),
        record_danmu: config.record_danmu,
        cookies: config.cookies,
        platform_specific_config: config.platform_specific_config,
        proxy_config: config.proxy_config,
        output_folder: config.output_folder,
        output_filename_template: config.output_filename_template,
        download_engine: config.download_engine,
        stream_selection_config: config.stream_selection_config,
        output_file_format: config.output_file_format,
        min_segment_size_bytes: config.min_segment_size_bytes.map(|v| v as u64),
        max_download_duration_secs: config.max_download_duration_secs.map(|v| v as u64),
        max_part_size_bytes: config.max_part_size_bytes.map(|v| v as u64),
        download_retry_policy: config.download_retry_policy,
        event_hooks: config.event_hooks,
        pipeline: config.pipeline,
        session_complete_pipeline: config.session_complete_pipeline,
        paired_segment_pipeline: config.paired_segment_pipeline,
    }
}

#[utoipa::path(
    get,
    path = "/api/config/global",
    tag = "config",
    responses(
        (status = 200, description = "Global configuration", body = GlobalConfigResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_global_config(
    State(state): State<AppState>,
) -> ApiResult<Json<GlobalConfigResponse>> {
    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::internal("ConfigService not available"))?;

    let config = config_service
        .get_global_config()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to get global config: {}", e)))?;

    Ok(Json(map_global_config_to_response(config)))
}

#[utoipa::path(
    patch,
    path = "/api/config/global",
    tag = "config",
    request_body = UpdateGlobalConfigRequest,
    responses(
        (status = 200, description = "Configuration updated", body = GlobalConfigResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_global_config(
    State(state): State<AppState>,
    Json(request): Json<UpdateGlobalConfigRequest>,
) -> ApiResult<Json<GlobalConfigResponse>> {
    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::internal("ConfigService not available"))?;

    tracing::info!(
        ?request,
        "Received request to update global configuration via API"
    );

    // Get current config to apply partial updates
    let mut config = config_service
        .get_global_config()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to get global config: {}", e)))?;

    let mut updated_fields: Vec<&'static str> = Vec::new();

    debug!(
        pipeline_before = ?config.pipeline,
        "Global config pipeline value BEFORE apply_updates"
    );

    apply_updates!(config, request, updated_fields; [
        output_folder: |v: serde_json::Value| v.as_str().map(String::from),
        output_filename_template: |v: serde_json::Value| v.as_str().map(String::from),
        output_file_format: |v: serde_json::Value| v.as_str().map(String::from),
        min_segment_size_bytes: |v: serde_json::Value| v.as_i64(),
        max_download_duration_secs: |v: serde_json::Value| v.as_i64(),
        max_part_size_bytes: |v: serde_json::Value| v.as_i64(),
        max_concurrent_downloads: |v: serde_json::Value| v.as_i64().map(|n| n as i32),
        max_concurrent_uploads: |v: serde_json::Value| v.as_i64().map(|n| n as i32),
        max_concurrent_cpu_jobs: |v: serde_json::Value| v.as_i64().map(|n| n as i32),
        max_concurrent_io_jobs: |v: serde_json::Value| v.as_i64().map(|n| n as i32),
        streamer_check_delay_ms: |v: serde_json::Value| v.as_i64(),
        offline_check_delay_ms: |v: serde_json::Value| v.as_i64(),
        offline_check_count: |v: serde_json::Value| v.as_i64().map(|n| n as i32),
        job_history_retention_days: |v: serde_json::Value| v.as_i64().map(|n| n as i32),
        notification_event_log_retention_days: |v: serde_json::Value| v.as_i64().map(|n| n as i32),
        session_gap_time_secs: |v: serde_json::Value| v.as_i64(),
        default_download_engine: |v: serde_json::Value| v.as_str().map(String::from),
        record_danmu: |v: serde_json::Value| v.as_bool(),
        proxy_config: |v: serde_json::Value| v.as_str().map(String::from),
        pipeline: |v: serde_json::Value| v.as_str().map(String::from),
        session_complete_pipeline: |v: serde_json::Value| v.as_str().map(String::from),
        paired_segment_pipeline: |v: serde_json::Value| v.as_str().map(String::from),
        auto_thumbnail: |v: serde_json::Value| v.as_bool(),
    ]);

    debug!(
        pipeline_after = ?config.pipeline,
        updated_fields = ?updated_fields,
        "Global config pipeline value AFTER apply_updates"
    );

    let updated_fields_summary = if updated_fields.is_empty() {
        "none".to_string()
    } else {
        updated_fields.join(", ")
    };

    // Update config (cache invalidation is handled automatically by ConfigService)
    if let Err(e) = config_service.update_global_config(&config).await {
        tracing::error!(
            error = %e,
            updated_fields = %updated_fields_summary,
            "Failed to update global config via API"
        );
        return Err(ApiError::internal(format!(
            "Failed to update global config: {}",
            e
        )));
    }

    tracing::info!(
        updated_fields = %updated_fields_summary,
        "Global configuration updated successfully via API"
    );

    Ok(Json(map_global_config_to_response(config)))
}

#[utoipa::path(
    get,
    path = "/api/config/platforms",
    tag = "config",
    responses(
        (status = 200, description = "List of platform configurations", body = Vec<PlatformConfigResponse>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_platform_configs(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<PlatformConfigResponse>>> {
    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::internal("ConfigService not available"))?;

    let configs = config_service
        .list_platform_configs()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to list platform configs: {}", e)))?;

    let responses: Vec<PlatformConfigResponse> = configs
        .into_iter()
        .map(map_platform_config_to_response)
        .collect();

    Ok(Json(responses))
}

#[utoipa::path(
    get,
    path = "/api/config/platforms/{id}",
    tag = "config",
    params(("id" = String, Path, description = "Platform config ID")),
    responses(
        (status = 200, description = "Platform configuration", body = PlatformConfigResponse),
        (status = 404, description = "Not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_platform_config(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<PlatformConfigResponse>> {
    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::internal("ConfigService not available"))?;

    let config = config_service.get_platform_config(&id).await.map_err(|e| {
        if e.to_string().contains("not found") {
            ApiError::not_found(format!("Platform config with id '{}' not found", id))
        } else {
            ApiError::internal(format!("Failed to get platform config: {}", e))
        }
    })?;

    Ok(Json(map_platform_config_to_response(config)))
}

#[utoipa::path(
    put,
    path = "/api/config/platforms/{id}",
    tag = "config",
    params(("id" = String, Path, description = "Platform config ID")),
    request_body = PlatformConfigResponse,
    responses(
        (status = 200, description = "Platform configuration updated", body = PlatformConfigResponse),
        (status = 400, description = "Bad request", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn replace_platform_config(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<PlatformConfigResponse>,
) -> ApiResult<Json<PlatformConfigResponse>> {
    tracing::info!(
        platform_id = %id,
        ?request,
        "Received request to replace platform configuration"
    );

    if request.id != id {
        return Err(ApiError::bad_request("Path ID does not match body ID"));
    }

    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::internal("ConfigService not available"))?;

    // Build the full config model from request
    let config = PlatformConfigDbModel {
        id: request.id,
        platform_name: request.name,
        fetch_delay_ms: request.fetch_delay_ms.map(|v| v as i64),
        download_delay_ms: request.download_delay_ms.map(|v| v as i64),
        record_danmu: request.record_danmu,
        cookies: request.cookies,
        platform_specific_config: request.platform_specific_config,
        proxy_config: request.proxy_config,
        output_folder: request.output_folder,
        output_filename_template: request.output_filename_template,
        download_engine: request.download_engine,
        stream_selection_config: request.stream_selection_config,
        output_file_format: request.output_file_format,
        min_segment_size_bytes: request.min_segment_size_bytes.map(|v| v as i64),
        max_download_duration_secs: request.max_download_duration_secs.map(|v| v as i64),
        max_part_size_bytes: request.max_part_size_bytes.map(|v| v as i64),
        download_retry_policy: request.download_retry_policy,
        event_hooks: request.event_hooks,
        pipeline: request.pipeline,
        session_complete_pipeline: request.session_complete_pipeline,
        paired_segment_pipeline: request.paired_segment_pipeline,
    };

    // Replace config
    if let Err(e) = config_service.update_platform_config(&config).await {
        tracing::error!(
            platform_id = %id,
            error = %e,
            "Failed to replace platform config"
        );
        return Err(ApiError::internal(format!(
            "Failed to replace platform config: {}",
            e
        )));
    }

    tracing::info!(
        platform_id = %id,
        "Platform configuration replaced successfully"
    );

    Ok(Json(map_platform_config_to_response(config)))
}

#[cfg(test)]
mod tests {

    use crate::api::models::GlobalConfigResponse;

    #[test]
    fn test_global_config_response_serialization() {
        let response = GlobalConfigResponse {
            output_folder: "/app/output".to_string(),
            output_filename_template: "{name}".to_string(),
            output_file_format: "flv".to_string(),
            min_segment_size_bytes: 1048576,
            max_download_duration_secs: 0,
            max_part_size_bytes: 8589934592,
            record_danmu: false,
            max_concurrent_downloads: 6,
            max_concurrent_uploads: 3,
            streamer_check_delay_ms: 60000,
            proxy_config: None,
            offline_check_delay_ms: 20000,
            offline_check_count: 3,
            default_download_engine: "mesio".to_string(),
            max_concurrent_cpu_jobs: 0,
            max_concurrent_io_jobs: 8,
            job_history_retention_days: 30,
            notification_event_log_retention_days: 30,
            session_gap_time_secs: 3600,
            pipeline: None,
            session_complete_pipeline: None,
            paired_segment_pipeline: None,
            log_filter_directive: "rust_srec=info".to_string(),
            auto_thumbnail: true,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("downloads"));
        assert!(json.contains("mesio"));
    }
}

#[cfg(test)]
mod property_tests {
    use super::ApplyUpdate;
    use crate::api::models::UpdateGlobalConfigRequest;
    use crate::database::models::GlobalConfigDbModel;
    use proptest::prelude::*;

    /// Apply partial updates from UpdateGlobalConfigRequest to GlobalConfigDbModel
    fn apply_global_config_update(
        config: &mut GlobalConfigDbModel,
        request: &UpdateGlobalConfigRequest,
    ) {
        // Clone request to use with macro (which expects owned fields)
        let request = request.clone();
        let mut updated_fields: Vec<&'static str> = Vec::new();

        apply_updates!(
            config, request, updated_fields; [
                output_folder: |v: serde_json::Value| v.as_str().map(String::from),
                output_filename_template: |v: serde_json::Value| v.as_str().map(String::from),
                output_file_format: |v: serde_json::Value| v.as_str().map(String::from),
                min_segment_size_bytes: |v: serde_json::Value| v.as_i64(),
                max_download_duration_secs: |v: serde_json::Value| v.as_i64(),
                max_part_size_bytes: |v: serde_json::Value| v.as_i64(),
                max_concurrent_downloads: |v: serde_json::Value| v.as_i64().map(|n| n as i32),
                max_concurrent_uploads: |v: serde_json::Value| v.as_i64().map(|n| n as i32),
                max_concurrent_cpu_jobs: |v: serde_json::Value| v.as_i64().map(|n| n as i32),
                max_concurrent_io_jobs: |v: serde_json::Value| v.as_i64().map(|n| n as i32),
                streamer_check_delay_ms: |v: serde_json::Value| v.as_i64(),
                offline_check_delay_ms: |v: serde_json::Value| v.as_i64(),
                offline_check_count: |v: serde_json::Value| v.as_i64().map(|n| n as i32),
                job_history_retention_days: |v: serde_json::Value| v.as_i64().map(|n| n as i32),
                notification_event_log_retention_days: |v: serde_json::Value| v.as_i64().map(|n| n as i32),
                session_gap_time_secs: |v: serde_json::Value| v.as_i64(),
                default_download_engine: |v: serde_json::Value| v.as_str().map(String::from),
                record_danmu: |v: serde_json::Value| v.as_bool(),
                proxy_config: |v: serde_json::Value| v.as_str().map(String::from),
                pipeline: |v: serde_json::Value| v.as_str().map(String::from),
                session_complete_pipeline: |v: serde_json::Value| v.as_str().map(String::from),
                paired_segment_pipeline: |v: serde_json::Value| v.as_str().map(String::from)
            ]
        );
    }

    /// Strategy for generating valid output folder paths
    fn output_folder_strategy() -> impl Strategy<Value = String> {
        prop::string::string_regex(r"\.?/?[a-zA-Z0-9_-]{1,20}(/[a-zA-Z0-9_-]{1,20}){0,3}")
            .unwrap()
            .prop_filter("non-empty folder", |s| !s.is_empty())
    }

    /// Strategy for generating valid filename templates
    fn filename_template_strategy() -> impl Strategy<Value = String> {
        prop::string::string_regex(r"\{[a-zA-Z_]+\}(-\{[a-zA-Z_%]+\}){0,3}")
            .unwrap()
            .prop_filter("non-empty template", |s| !s.is_empty())
    }

    /// Strategy for generating valid file formats
    fn file_format_strategy() -> impl Strategy<Value = String> {
        prop::sample::select(vec![
            "flv".to_string(),
            "mp4".to_string(),
            "ts".to_string(),
            "mkv".to_string(),
        ])
    }

    /// Strategy for generating valid download engine names
    fn download_engine_strategy() -> impl Strategy<Value = String> {
        prop::sample::select(vec![
            "ffmpeg".to_string(),
            "mesio".to_string(),
            "streamlink".to_string(),
        ])
    }

    /// Strategy for generating valid proxy config JSON
    fn proxy_config_strategy() -> impl Strategy<Value = String> {
        prop::string::string_regex(
            r#"\{"enabled": (true|false), "use_system_proxy": (true|false)\}"#,
        )
        .unwrap()
    }

    // **Feature: jwt-auth-and-api-implementation, Property 8: Config Update Persistence**
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        /// Property: For any valid global config update, applying the update then reading
        /// the config should reflect the updated values.
        #[test]
        fn prop_global_config_update_persistence(
            // Generate optional update fields
            output_folder in prop::option::of(output_folder_strategy()),
            output_filename_template in prop::option::of(filename_template_strategy()),
            output_file_format in prop::option::of(file_format_strategy()),
            min_segment_size_bytes in prop::option::of(1024u64..16777216u64),
            max_download_duration_secs in prop::option::of(0u64..86400u64),
            max_part_size_bytes in prop::option::of(1048576u64..17179869184u64),
            max_concurrent_downloads in prop::option::of(1u32..100u32),
            max_concurrent_uploads in prop::option::of(1u32..50u32),
            max_concurrent_cpu_jobs in prop::option::of(0u32..32u32),
            max_concurrent_io_jobs in prop::option::of(1u32..64u32),
            streamer_check_delay_ms in prop::option::of(1000u64..300000u64),
            offline_check_delay_ms in prop::option::of(1000u64..120000u64),
            offline_check_count in prop::option::of(1u32..10u32),
            job_history_retention_days in prop::option::of(1u32..365u32),
            notification_event_log_retention_days in prop::option::of(1u32..365u32),
            default_download_engine in prop::option::of(download_engine_strategy()),
            record_danmu in prop::option::of(prop::bool::ANY),
            proxy_config in prop::option::of(proxy_config_strategy()),
        ) {
            // Start with default config
            let mut config = GlobalConfigDbModel::default();
            let original_config = config.clone();

            // Create update request with generated values
            let update_request = UpdateGlobalConfigRequest {
                output_folder: output_folder.clone().map(|v| serde_json::json!(v)),
                output_filename_template: output_filename_template.clone().map(|v| serde_json::json!(v)),
                output_file_format: output_file_format.clone().map(|v| serde_json::json!(v)),
                min_segment_size_bytes: min_segment_size_bytes.map(|v| serde_json::json!(v)),
                max_download_duration_secs: max_download_duration_secs.map(|v| serde_json::json!(v)),
                max_part_size_bytes: max_part_size_bytes.map(|v| serde_json::json!(v)),
                max_concurrent_downloads: max_concurrent_downloads.map(|v| serde_json::json!(v)),
                max_concurrent_uploads: max_concurrent_uploads.map(|v| serde_json::json!(v)),
                max_concurrent_cpu_jobs: max_concurrent_cpu_jobs.map(|v| serde_json::json!(v)),
                max_concurrent_io_jobs: max_concurrent_io_jobs.map(|v| serde_json::json!(v)),
                streamer_check_delay_ms: streamer_check_delay_ms.map(|v| serde_json::json!(v)),
                offline_check_delay_ms: offline_check_delay_ms.map(|v| serde_json::json!(v)),
                offline_check_count: offline_check_count.map(|v| serde_json::json!(v)),
                job_history_retention_days: job_history_retention_days.map(|v| serde_json::json!(v)),
                notification_event_log_retention_days: notification_event_log_retention_days.map(|v| serde_json::json!(v)),
                default_download_engine: default_download_engine.clone().map(|v| serde_json::json!(v)),
                record_danmu: record_danmu.map(|v| serde_json::json!(v)),
                proxy_config: proxy_config.clone().map(|v| serde_json::json!(v)),
                session_gap_time_secs: None,
                pipeline: None,
                session_complete_pipeline: None,
                paired_segment_pipeline: None,
                log_filter_directive: None,
                auto_thumbnail: None,
            };

            // Apply the update
            apply_global_config_update(&mut config, &update_request);

            // Property: Each updated field should reflect the new value
            if let Some(ref folder) = output_folder {
                prop_assert_eq!(&config.output_folder, folder, "output_folder should be updated");
            } else {
                prop_assert_eq!(&config.output_folder, &original_config.output_folder, "output_folder should remain unchanged");
            }

            if let Some(ref template) = output_filename_template {
                prop_assert_eq!(&config.output_filename_template, template, "output_filename_template should be updated");
            } else {
                prop_assert_eq!(&config.output_filename_template, &original_config.output_filename_template, "output_filename_template should remain unchanged");
            }

            if let Some(ref format) = output_file_format {
                prop_assert_eq!(&config.output_file_format, format, "output_file_format should be updated");
            } else {
                prop_assert_eq!(&config.output_file_format, &original_config.output_file_format, "output_file_format should remain unchanged");
            }

            if let Some(min_segment_size_bytes_value) = min_segment_size_bytes {
                prop_assert_eq!(config.min_segment_size_bytes, min_segment_size_bytes_value as i64, "min_segment_size_bytes should be updated");
            } else {
                prop_assert_eq!(config.min_segment_size_bytes, original_config.min_segment_size_bytes, "min_segment_size_bytes should remain unchanged");
            }

            if let Some(max_download_duration_secs_value) = max_download_duration_secs {
                prop_assert_eq!(config.max_download_duration_secs, max_download_duration_secs_value as i64, "max_download_duration_secs should be updated");
            } else {
                prop_assert_eq!(config.max_download_duration_secs, original_config.max_download_duration_secs, "max_download_duration_secs should remain unchanged");
            }

            if let Some(max_part_size_bytes_value) = max_part_size_bytes {
                prop_assert_eq!(config.max_part_size_bytes, max_part_size_bytes_value as i64, "max_part_size_bytes should be updated");
            } else {
                prop_assert_eq!(config.max_part_size_bytes, original_config.max_part_size_bytes, "max_part_size_bytes should remain unchanged");
            }

            if let Some(downloads) = max_concurrent_downloads {
                prop_assert_eq!(config.max_concurrent_downloads, downloads as i32, "max_concurrent_downloads should be updated");
            } else {
                prop_assert_eq!(config.max_concurrent_downloads, original_config.max_concurrent_downloads, "max_concurrent_downloads should remain unchanged");
            }

            if let Some(uploads) = max_concurrent_uploads {
                prop_assert_eq!(config.max_concurrent_uploads, uploads as i32, "max_concurrent_uploads should be updated");
            } else {
                prop_assert_eq!(config.max_concurrent_uploads, original_config.max_concurrent_uploads, "max_concurrent_uploads should remain unchanged");
            }

            if let Some(cpu_jobs) = max_concurrent_cpu_jobs {
                prop_assert_eq!(config.max_concurrent_cpu_jobs, cpu_jobs as i32, "max_concurrent_cpu_jobs should be updated");
            } else {
                prop_assert_eq!(config.max_concurrent_cpu_jobs, original_config.max_concurrent_cpu_jobs, "max_concurrent_cpu_jobs should remain unchanged");
            }

            if let Some(io_jobs) = max_concurrent_io_jobs {
                prop_assert_eq!(config.max_concurrent_io_jobs, io_jobs as i32, "max_concurrent_io_jobs should be updated");
            } else {
                prop_assert_eq!(config.max_concurrent_io_jobs, original_config.max_concurrent_io_jobs, "max_concurrent_io_jobs should remain unchanged");
            }

            if let Some(check_delay) = streamer_check_delay_ms {
                prop_assert_eq!(config.streamer_check_delay_ms, check_delay as i64, "streamer_check_delay_ms should be updated");
            } else {
                prop_assert_eq!(config.streamer_check_delay_ms, original_config.streamer_check_delay_ms, "streamer_check_delay_ms should remain unchanged");
            }

            if let Some(offline_delay) = offline_check_delay_ms {
                prop_assert_eq!(config.offline_check_delay_ms, offline_delay as i64, "offline_check_delay_ms should be updated");
            } else {
                prop_assert_eq!(config.offline_check_delay_ms, original_config.offline_check_delay_ms, "offline_check_delay_ms should remain unchanged");
            }

            if let Some(offline_count) = offline_check_count {
                prop_assert_eq!(config.offline_check_count, offline_count as i32, "offline_check_count should be updated");
            } else {
                prop_assert_eq!(config.offline_check_count, original_config.offline_check_count, "offline_check_count should remain unchanged");
            }

            if let Some(job_history) = job_history_retention_days {
                prop_assert_eq!(config.job_history_retention_days, job_history as i32, "job_history_retention_days should be updated");
            } else {
                prop_assert_eq!(config.job_history_retention_days, original_config.job_history_retention_days, "job_history_retention_days should remain unchanged");
            }

            if let Some(notification_retention) = notification_event_log_retention_days {
                prop_assert_eq!(config.notification_event_log_retention_days, notification_retention as i32, "notification_event_log_retention_days should be updated");
            } else {
                prop_assert_eq!(config.notification_event_log_retention_days, original_config.notification_event_log_retention_days, "notification_event_log_retention_days should remain unchanged");
            }

            if let Some(ref engine) = default_download_engine {
                prop_assert_eq!(&config.default_download_engine, engine, "default_download_engine should be updated");
            } else {
                prop_assert_eq!(&config.default_download_engine, &original_config.default_download_engine, "default_download_engine should remain unchanged");
            }

            if let Some(danmu) = record_danmu {
                prop_assert_eq!(config.record_danmu, danmu, "record_danmu should be updated");
            } else {
                prop_assert_eq!(config.record_danmu, original_config.record_danmu, "record_danmu should remain unchanged");
            }

            if let Some(ref proxy) = proxy_config {
                prop_assert_eq!(&config.proxy_config, proxy, "proxy_config should be updated");
            } else {
                prop_assert_eq!(&config.proxy_config, &original_config.proxy_config, "proxy_config should remain unchanged");
            }
        }


    }
}
