//! Configuration routes.

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, patch},
};

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{
    GlobalConfigResponse, PlatformConfigResponse, UpdateGlobalConfigRequest,
    UpdatePlatformConfigRequest,
};
use crate::api::server::AppState;
use crate::database::models::{GlobalConfigDbModel, PlatformConfigDbModel};

/// Helper macro to apply optional updates from requests.
macro_rules! apply_updates {
    // Form 1: With tracker (for Global Config)
    ($target:ident, $source:ident, $tracker:ident; [
        $( $field:ident $(: $transform:expr)? ),* $(,)?
    ]) => {
        $(
            if let Some(val) = $source.$field {
                // Apply transformation if provided, otherwise direct assignment
                $target.$field = apply_updates!(@val val, $($transform)?);
                $tracker.push(stringify!($field));
            }
        )*
    };

    // Form 2: Without tracker (for Platform Config)
    ($target:ident, $source:ident; [
        $( $field:ident $(: $transform:expr)? ),* $(,)?
    ]) => {
        $(
            if let Some(val) = $source.$field {
                $target.$field = apply_updates!(@val val, $($transform)?);
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
        .route("/platforms/{id}", patch(update_platform_config))
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
    }
}

/// Get global configuration.
async fn get_global_config(State(state): State<AppState>) -> ApiResult<Json<GlobalConfigResponse>> {
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

/// Update global configuration.
async fn update_global_config(
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

    apply_updates!(config, request, updated_fields; [
        output_folder,
        output_filename_template,
        output_file_format,
        min_segment_size_bytes: |v| v as i64,
        max_download_duration_secs: |v| v as i64,
        max_part_size_bytes: |v| v as i64,
        max_concurrent_downloads: |v| v as i32,
        max_concurrent_uploads: |v| v as i32,
        max_concurrent_cpu_jobs: |v| v as i32,
        max_concurrent_io_jobs: |v| v as i32,
        streamer_check_delay_ms: |v| v as i64,
        offline_check_delay_ms: |v| v as i64,
        offline_check_count: |v| v as i32,
        job_history_retention_days: |v| v as i32,
        default_download_engine,
        record_danmu,
        proxy_config
    ]);

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

/// List all platform configurations.
async fn list_platform_configs(
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

/// Get a specific platform configuration.
async fn get_platform_config(
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

/// Update a platform configuration.
async fn update_platform_config(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdatePlatformConfigRequest>,
) -> ApiResult<Json<PlatformConfigResponse>> {
    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::internal("ConfigService not available"))?;

    // Get current config to apply partial updates
    let mut config = config_service.get_platform_config(&id).await.map_err(|e| {
        if e.to_string().contains("not found") {
            ApiError::not_found(format!("Platform config with id '{}' not found", id))
        } else {
            ApiError::internal(format!("Failed to get platform config: {}", e))
        }
    })?;

    apply_updates!(config, request; [
        fetch_delay_ms: |v| Some(v as i64),
        download_delay_ms: |v| Some(v as i64),
        record_danmu: |v| Some(v),
        cookies: |v| Some(v),
        platform_specific_config: |v| Some(v),
        proxy_config: |v| Some(v),
        output_folder: |v| Some(v),
        output_filename_template: |v| Some(v),
        download_engine: |v| Some(v),
        stream_selection_config: |v| Some(v),
        output_file_format: |v| Some(v),
        min_segment_size_bytes: |v| Some(v as i64),
        max_download_duration_secs: |v| Some(v as i64),
        max_part_size_bytes: |v| Some(v as i64),
        download_retry_policy: |v| Some(v),
        event_hooks: |v| Some(v)
    ]);

    // Update config (cache invalidation is handled automatically by ConfigService)
    config_service
        .update_platform_config(&config)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to update platform config: {}", e)))?;

    Ok(Json(map_platform_config_to_response(config)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_global_config_response_serialization() {
        let response = GlobalConfigResponse {
            output_folder: "./downloads".to_string(),
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
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("downloads"));
        assert!(json.contains("mesio"));
    }
}

#[cfg(test)]
mod property_tests {
    use crate::api::models::{UpdateGlobalConfigRequest, UpdatePlatformConfigRequest};
    use crate::database::models::{GlobalConfigDbModel, PlatformConfigDbModel};
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
                output_folder,
                output_filename_template,
                output_file_format,
                min_segment_size_bytes: |v| v as i64,
                max_download_duration_secs: |v| v as i64,
                max_part_size_bytes: |v| v as i64,
                max_concurrent_downloads: |v| v as i32,
                max_concurrent_uploads: |v| v as i32,
                max_concurrent_cpu_jobs: |v| v as i32,
                max_concurrent_io_jobs: |v| v as i32,
                streamer_check_delay_ms: |v| v as i64,
                offline_check_delay_ms: |v| v as i64,
                offline_check_count: |v| v as i32,
                job_history_retention_days: |v| v as i32,
                default_download_engine,
                record_danmu,
                proxy_config
            ]
        );
    }

    /// Apply partial updates from UpdatePlatformConfigRequest to PlatformConfigDbModel
    fn apply_platform_config_update(
        config: &mut PlatformConfigDbModel,
        request: &UpdatePlatformConfigRequest,
    ) {
        // Clone request to use with macro
        let request = request.clone();

        apply_updates!(config, request; [
            fetch_delay_ms: |v| Some(v as i64),
            download_delay_ms: |v| Some(v as i64),
            record_danmu: |v| Some(v),
            cookies: |v| Some(v),
            platform_specific_config: |v| Some(v),
            proxy_config: |v| Some(v),
            output_folder: |v| Some(v),
            output_filename_template: |v| Some(v),
            download_engine: |v| Some(v),
            stream_selection_config: |v| Some(v),
            output_file_format: |v| Some(v),
            min_segment_size_bytes: |v| Some(v as i64),
            max_download_duration_secs: |v| Some(v as i64),
            max_part_size_bytes: |v| Some(v as i64),
            download_retry_policy: |v| Some(v),
            event_hooks: |v| Some(v)
        ]);
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

    /// Strategy for generating valid cookie strings
    fn cookies_strategy() -> impl Strategy<Value = String> {
        prop::string::string_regex(r"[a-zA-Z0-9_]+=[a-zA-Z0-9_]+")
            .unwrap()
            .prop_filter("non-empty cookies", |s| !s.is_empty())
    }

    /// Strategy for generating valid proxy config JSON
    fn proxy_config_strategy() -> impl Strategy<Value = String> {
        prop::string::string_regex(
            r#"\{"enabled": (true|false), "use_system_proxy": (true|false)\}"#,
        )
        .unwrap()
    }

    // **Feature: jwt-auth-and-api-implementation, Property 8: Config Update Persistence**
    // **Validates: Requirements 3.2, 3.5**
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
            default_download_engine in prop::option::of(download_engine_strategy()),
            record_danmu in prop::option::of(prop::bool::ANY),
            proxy_config in prop::option::of(proxy_config_strategy()),
        ) {
            // Start with default config
            let mut config = GlobalConfigDbModel::default();
            let original_config = config.clone();

            // Create update request with generated values
            let update_request = UpdateGlobalConfigRequest {
                output_folder: output_folder.clone(),
                output_filename_template: output_filename_template.clone(),
                output_file_format: output_file_format.clone(),
                min_segment_size_bytes,
                max_download_duration_secs,
                max_part_size_bytes,
                max_concurrent_downloads,
                max_concurrent_uploads,
                max_concurrent_cpu_jobs,
                max_concurrent_io_jobs,
                streamer_check_delay_ms,
                offline_check_delay_ms,
                offline_check_count,
                job_history_retention_days,
                default_download_engine: default_download_engine.clone(),
                record_danmu,
                proxy_config: proxy_config.clone(),
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

        /// Property: For any valid platform config update, applying the update then reading
        /// the config should reflect the updated values.
        #[test]
        fn prop_platform_config_update_persistence(
            // Generate platform config base
            platform_id in "[a-zA-Z0-9_-]{1,20}",
            platform_name in "[a-zA-Z0-9_]{1,20}",
            initial_fetch_delay in 1000i64..60000i64,
            initial_download_delay in 1000i64..60000i64,
            // Generate optional update fields
            fetch_delay_ms in prop::option::of(1000u64..120000u64),
            download_delay_ms in prop::option::of(1000u64..120000u64),
            record_danmu in prop::option::of(prop::bool::ANY),
            cookies in prop::option::of(cookies_strategy()),
            platform_specific_config in prop::option::of(prop::string::string_regex(r#"\{"key": "value"\}"#).unwrap()),
            proxy_config in prop::option::of(proxy_config_strategy()),
            output_folder in prop::option::of(output_folder_strategy()),
            output_filename_template in prop::option::of(filename_template_strategy()),
            download_engine in prop::option::of(download_engine_strategy()),
            stream_selection_config in prop::option::of(prop::string::string_regex(r#"\{"mode": "auto"\}"#).unwrap()),
            output_file_format in prop::option::of(file_format_strategy()),
            min_segment_size_bytes in prop::option::of(1024u64..10485760u64),
            max_download_duration_secs in prop::option::of(60u64..3600u64),
            max_part_size_bytes in prop::option::of(1048576u64..1073741824u64),
            download_retry_policy in prop::option::of(prop::string::string_regex(r#"\{"max_retries": 5\}"#).unwrap()),
            event_hooks in prop::option::of(prop::string::string_regex(r#"\{"on_download_start": \[\]\}"#).unwrap()),
        ) {
            // Create initial platform config
            let mut config = PlatformConfigDbModel {
                id: platform_id,
                platform_name,
                fetch_delay_ms: Some(initial_fetch_delay),
                download_delay_ms: Some(initial_download_delay),
                cookies: None,
                platform_specific_config: None,
                proxy_config: None,
                record_danmu: None,
                output_folder: None,
                output_filename_template: None,
                download_engine: None,
                stream_selection_config: None,
                output_file_format: None,
                min_segment_size_bytes: None,
                max_download_duration_secs: None,
                max_part_size_bytes: None,
                download_retry_policy: None,
                event_hooks: None,
                pipeline: None,
            };
            let original_config = config.clone();

            // Create update request with generated values
            let update_request = UpdatePlatformConfigRequest {
                fetch_delay_ms,
                download_delay_ms,
                record_danmu,
                cookies: cookies.clone(),
                platform_specific_config: platform_specific_config.clone(),
                proxy_config: proxy_config.clone(),
                output_folder: output_folder.clone(),
                output_filename_template: output_filename_template.clone(),
                download_engine: download_engine.clone(),
                stream_selection_config: stream_selection_config.clone(),
                output_file_format: output_file_format.clone(),
                min_segment_size_bytes,
                max_download_duration_secs,
                max_part_size_bytes,
                download_retry_policy: download_retry_policy.clone(),
                event_hooks: event_hooks.clone(),
            };

            // Apply the update
            apply_platform_config_update(&mut config, &update_request);

            // Property: Each updated field should reflect the new value
            if let Some(fetch_delay) = fetch_delay_ms {
                prop_assert_eq!(config.fetch_delay_ms, Some(fetch_delay as i64), "fetch_delay_ms should be updated");
            } else {
                prop_assert_eq!(config.fetch_delay_ms, original_config.fetch_delay_ms, "fetch_delay_ms should remain unchanged");
            }

            if let Some(download_delay) = download_delay_ms {
                prop_assert_eq!(config.download_delay_ms, Some(download_delay as i64), "download_delay_ms should be updated");
            } else {
                prop_assert_eq!(config.download_delay_ms, original_config.download_delay_ms, "download_delay_ms should remain unchanged");
            }

            if let Some(danmu) = record_danmu {
                prop_assert_eq!(config.record_danmu, Some(danmu), "record_danmu should be updated");
            } else {
                prop_assert_eq!(config.record_danmu, original_config.record_danmu, "record_danmu should remain unchanged");
            }

            if let Some(ref cookie_val) = cookies {
                prop_assert_eq!(config.cookies.as_ref(), Some(cookie_val), "cookies should be updated");
            } else {
                prop_assert_eq!(config.cookies, original_config.cookies, "cookies should remain unchanged");
            }

            if let Some(ref psc_val) = platform_specific_config {
                prop_assert_eq!(config.platform_specific_config.as_ref(), Some(psc_val), "platform_specific_config should be updated");
            } else {
                prop_assert_eq!(config.platform_specific_config, original_config.platform_specific_config, "platform_specific_config should remain unchanged");
            }

            if let Some(ref proxy_val) = proxy_config {
                prop_assert_eq!(config.proxy_config.as_ref(), Some(proxy_val), "proxy_config should be updated");
            } else {
                prop_assert_eq!(config.proxy_config, original_config.proxy_config, "proxy_config should remain unchanged");
            }

            if let Some(ref output_folder_val) = output_folder {
                prop_assert_eq!(config.output_folder.as_ref(), Some(output_folder_val), "output_folder should be updated");
            } else {
                prop_assert_eq!(config.output_folder, original_config.output_folder, "output_folder should remain unchanged");
            }

            if let Some(ref output_filename_template_val) = output_filename_template {
                prop_assert_eq!(config.output_filename_template.as_ref(), Some(output_filename_template_val), "output_filename_template should be updated");
            } else {
                prop_assert_eq!(config.output_filename_template, original_config.output_filename_template, "output_filename_template should remain unchanged");
            }

            if let Some(ref download_engine_val) = download_engine {
                prop_assert_eq!(config.download_engine.as_ref(), Some(download_engine_val), "download_engine should be updated");
            } else {
                prop_assert_eq!(config.download_engine, original_config.download_engine, "download_engine should remain unchanged");
            }



            if let Some(ref stream_selection_config_val) = stream_selection_config {
                prop_assert_eq!(config.stream_selection_config.as_ref(), Some(stream_selection_config_val), "stream_selection_config should be updated");
            } else {
                prop_assert_eq!(config.stream_selection_config, original_config.stream_selection_config, "stream_selection_config should remain unchanged");
            }

            if let Some(ref output_file_format_val) = output_file_format {
                prop_assert_eq!(config.output_file_format.as_ref(), Some(output_file_format_val), "output_file_format should be updated");
            } else {
                prop_assert_eq!(config.output_file_format, original_config.output_file_format, "output_file_format should remain unchanged");
            }

            if let Some(min_segment_size_bytes_val) = min_segment_size_bytes {
                prop_assert_eq!(config.min_segment_size_bytes, Some(min_segment_size_bytes_val as i64), "min_segment_size_bytes should be updated");
            } else {
                prop_assert_eq!(config.min_segment_size_bytes, original_config.min_segment_size_bytes, "min_segment_size_bytes should remain unchanged");
            }

            if let Some(max_download_duration_secs_val) = max_download_duration_secs {
                prop_assert_eq!(config.max_download_duration_secs, Some(max_download_duration_secs_val as i64), "max_download_duration_secs should be updated");
            } else {
                prop_assert_eq!(config.max_download_duration_secs, original_config.max_download_duration_secs, "max_download_duration_secs should remain unchanged");
            }

            if let Some(max_part_size_bytes_val) = max_part_size_bytes {
                prop_assert_eq!(config.max_part_size_bytes, Some(max_part_size_bytes_val as i64), "max_part_size_bytes should be updated");
            } else {
                prop_assert_eq!(config.max_part_size_bytes, original_config.max_part_size_bytes, "max_part_size_bytes should remain unchanged");
            }

            if let Some(ref download_retry_policy_val) = download_retry_policy {
                prop_assert_eq!(config.download_retry_policy.as_ref(), Some(download_retry_policy_val), "download_retry_policy should be updated");
            } else {
                prop_assert_eq!(config.download_retry_policy, original_config.download_retry_policy, "download_retry_policy should remain unchanged");
            }

            if let Some(ref event_hooks_val) = event_hooks {
                prop_assert_eq!(config.event_hooks.as_ref(), Some(event_hooks_val), "event_hooks should be updated");
            } else {
                prop_assert_eq!(config.event_hooks, original_config.event_hooks, "event_hooks should remain unchanged");
            }
        }
    }
}
