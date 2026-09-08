//! Configuration routes.

use axum::{
    Json, Router,
    extract::{FromRef, Path, State},
    routing::{get, patch, put},
};
use tracing::debug;

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{GlobalConfigResponse, PlatformConfigResponse, UpdateGlobalConfigRequest};
use crate::api::server::AppState;
use crate::database::models::{GlobalConfigDbModel, PlatformConfigDbModel, RetentionDays};

#[derive(Clone)]
pub struct ConfigRouteState {
    config_service: std::sync::Arc<
        crate::config::ConfigService<
            crate::database::repositories::config::SqlxConfigRepository,
            crate::database::repositories::streamer::SqlxStreamerRepository,
        >,
    >,
}

impl FromRef<AppState> for ConfigRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            config_service: state.config_service.clone(),
        }
    }
}

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
    // Record each supplied field name for the global configuration update log.
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

    // Helper to handle optional transform
    (@val $val:ident, $transform:expr) => { ($transform)($val) };
    (@val $val:ident,) => { $val };
}

pub(super) fn validate_retention_days(field: &'static str, days: i64) -> ApiResult<()> {
    RetentionDays::try_from(days)
        .map(|_| ())
        .map_err(|_| ApiError::bad_request(format!("{field} must be between 0 and {}", i32::MAX)))
}

fn validate_optional_retention_days(
    field: &'static str,
    value: Option<&serde_json::Value>,
) -> ApiResult<()> {
    let Some(value) = value else {
        return Ok(());
    };
    let Some(days) = value.as_i64() else {
        return Err(ApiError::bad_request(format!("{field} must be an integer")));
    };
    validate_retention_days(field, days)
}

fn validate_global_config_request(request: &UpdateGlobalConfigRequest) -> ApiResult<()> {
    // Zero disables recording limits/retention or selects automatic CPU/IO concurrency.
    // Timeouts and probe/freshness intervals retain their existing clamps below.
    macro_rules! integers {
        ($($field:ident: ($min:expr, $max:expr)),+ $(,)?) => {
            $(if let Some(value) = &request.$field {
                let number = value.as_i64().ok_or_else(|| ApiError::bad_request(
                    concat!(stringify!($field), " must be a signed 64-bit integer"),
                ))?;
                if !($min..=$max).contains(&number) {
                    return Err(ApiError::bad_request(format!(
                        "{} must be between {} and {}", stringify!($field), $min, $max,
                    )));
                }
            })+
        };
    }
    integers! {
        min_segment_size_bytes: (0, i64::MAX),
        max_download_duration_secs: (0, i64::MAX),
        max_part_size_bytes: (0, i64::MAX),
        max_concurrent_downloads: (1, i64::from(i32::MAX)),
        max_concurrent_uploads: (1, i64::from(i32::MAX)),
        max_concurrent_cpu_jobs: (0, i64::from(i32::MAX)),
        max_concurrent_io_jobs: (0, i64::from(i32::MAX)),
        streamer_check_delay_ms: (1, i64::MAX),
        offline_check_delay_ms: (1, i64::MAX),
        offline_check_count: (1, i64::from(i32::MAX)),
        pipeline_cpu_job_timeout_secs: (i64::MIN, i64::MAX),
        pipeline_io_job_timeout_secs: (i64::MIN, i64::MAX),
        pipeline_execute_timeout_secs: (i64::MIN, i64::MAX),
        queue_freshness_threshold_ms: (i64::MIN, i64::MAX),
        gpu_health_probe_interval_secs: (i64::MIN, i64::MAX),
    }
    validate_optional_retention_days(
        "job_history_retention_days",
        request.job_history_retention_days.as_ref(),
    )?;
    validate_optional_retention_days(
        "notification_event_log_retention_days",
        request.notification_event_log_retention_days.as_ref(),
    )?;
    // The GPU monitor constructs an Instant directly, unlike pipeline timeouts,
    // whose timer API handles durations beyond the host's representable range.
    if let Some(seconds) = request
        .gpu_health_probe_interval_secs
        .as_ref()
        .and_then(serde_json::Value::as_i64)
        && std::time::Instant::now()
            .checked_add(std::time::Duration::from_secs(
                seconds.max(1).unsigned_abs(),
            ))
            .is_none()
    {
        return Err(ApiError::bad_request(
            "gpu_health_probe_interval_secs exceeds the supported timer range",
        ));
    }

    macro_rules! typed_fields {
        ($predicate:ident, $description:literal; $($field:ident),+ $(,)?) => {
            $(if let Some(value) = &request.$field
                && !value.$predicate()
            {
                return Err(ApiError::bad_request(concat!(
                    stringify!($field), " must be ", $description,
                )));
            })+
        };
    }
    typed_fields!(is_string, "a string";
        output_folder, output_filename_template, output_file_format,
        default_download_engine, default_extractor, proxy_config, pipeline,
        session_complete_pipeline, paired_segment_pipeline,
    );
    typed_fields!(is_boolean, "a boolean";
        record_danmu, auto_thumbnail, stream_proxy_allow_private_targets,
    );
    Ok(())
}

fn platform_i64(field: &'static str, value: Option<u64>) -> ApiResult<Option<i64>> {
    value
        .map(|value| {
            i64::try_from(value)
                .map_err(|_| ApiError::bad_request(format!("{field} must be at most {}", i64::MAX)))
        })
        .transpose()
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
fn map_global_config_to_response(config: GlobalConfigDbModel) -> ApiResult<GlobalConfigResponse> {
    let job_history_retention_days = RetentionDays::try_from(config.job_history_retention_days)
        .map_err(|_| ApiError::internal("Stored job retention configuration is invalid"))?
        .as_u32();
    let notification_event_log_retention_days =
        RetentionDays::try_from(config.notification_event_log_retention_days)
            .map_err(|_| {
                ApiError::internal("Stored notification retention configuration is invalid")
            })?
            .as_u32();

    Ok(GlobalConfigResponse {
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
        default_extractor: config.default_extractor,
        record_danmu: config.record_danmu,
        danmu_statistics: config.danmu_statistics,
        job_history_retention_days,
        notification_event_log_retention_days,
        pipeline: config.pipeline,
        session_complete_pipeline: config.session_complete_pipeline,
        paired_segment_pipeline: config.paired_segment_pipeline,
        log_filter_directive: config.log_filter_directive,
        auto_thumbnail: config.auto_thumbnail,

        pipeline_cpu_job_timeout_secs: config.pipeline_cpu_job_timeout_secs.max(0) as u64,
        pipeline_io_job_timeout_secs: config.pipeline_io_job_timeout_secs.max(0) as u64,
        pipeline_execute_timeout_secs: config.pipeline_execute_timeout_secs.max(0) as u64,
        queue_freshness_threshold_ms: config.queue_freshness_threshold_ms.max(0) as u64,
        gpu_health_probe_interval_secs: config.gpu_health_probe_interval_secs.max(0) as u64,
        stream_proxy_allow_private_targets: config.stream_proxy_allow_private_targets,
    })
}

/// Map PlatformConfigDbModel to PlatformConfigResponse.
fn map_platform_config_to_response(config: PlatformConfigDbModel) -> PlatformConfigResponse {
    PlatformConfigResponse {
        id: config.id,
        name: config.platform_name,
        fetch_delay_ms: config.fetch_delay_ms.map(|v| v as u64),
        download_delay_ms: config.download_delay_ms.map(|v| v as u64),
        record_danmu: config.record_danmu,
        danmu_statistics: config.danmu_statistics,
        cookies: config.cookies,
        platform_specific_config: config.platform_specific_config,
        proxy_config: config.proxy_config,
        output_folder: config.output_folder,
        output_filename_template: config.output_filename_template,
        download_engine: config.download_engine,
        extractor: config.extractor,
        stream_selection_config: config.stream_selection_config,
        output_file_format: config.output_file_format,
        min_segment_size_bytes: config.min_segment_size_bytes.map(|v| v as u64),
        max_download_duration_secs: config.max_download_duration_secs.map(|v| v as u64),
        max_part_size_bytes: config.max_part_size_bytes.map(|v| v as u64),
        download_retry_policy: config.download_retry_policy,
        pipeline: config.pipeline,
        session_complete_pipeline: config.session_complete_pipeline,
        paired_segment_pipeline: config.paired_segment_pipeline,
        offline_check_count: config.offline_check_count.map(|v| v as u32),
        offline_check_delay_ms: config.offline_check_delay_ms.map(|v| v as u64),
    }
}

/// Validate the optional offline-check overrides on a request payload.
/// Mirrors the server-side floors enforced in
/// [`crate::session::HysteresisConfig::from_scheduler`].
fn validate_offline_check_overrides(
    count: Option<i64>,
    delay_ms: Option<i64>,
) -> Result<(), ApiError> {
    if let Some(c) = count
        && c < 1
    {
        return Err(ApiError::bad_request("offline_check_count must be >= 1"));
    }
    if let Some(d) = delay_ms
        && d < 1_000
    {
        return Err(ApiError::bad_request(
            "offline_check_delay_ms must be >= 1000",
        ));
    }
    Ok(())
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
    State(state): State<ConfigRouteState>,
) -> ApiResult<Json<GlobalConfigResponse>> {
    let config_service = &state.config_service;

    let config = config_service
        .get_global_config()
        .await
        .map_err(ApiError::from)?;

    Ok(Json(map_global_config_to_response(config)?))
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
    State(state): State<ConfigRouteState>,
    Json(request): Json<UpdateGlobalConfigRequest>,
) -> ApiResult<Json<GlobalConfigResponse>> {
    validate_global_config_request(&request)?;

    let config_service = &state.config_service;

    // The request body carries `proxy_config`, whose URL may embed proxy credentials;
    // only the field names are logged, via `updated_fields` after `apply_updates!` below.
    tracing::info!("Received request to update global configuration via API");

    // Get current config to apply partial updates
    let mut config = config_service
        .get_global_config()
        .await
        .map_err(ApiError::from)?;

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
        max_concurrent_downloads: |v: serde_json::Value| v.as_i64().and_then(|n| i32::try_from(n).ok()),
        max_concurrent_uploads: |v: serde_json::Value| v.as_i64().and_then(|n| i32::try_from(n).ok()),
        max_concurrent_cpu_jobs: |v: serde_json::Value| v.as_i64().and_then(|n| i32::try_from(n).ok()),
        max_concurrent_io_jobs: |v: serde_json::Value| v.as_i64().and_then(|n| i32::try_from(n).ok()),
        streamer_check_delay_ms: |v: serde_json::Value| v.as_i64(),
        offline_check_delay_ms: |v: serde_json::Value| v.as_i64(),
        offline_check_count: |v: serde_json::Value| v.as_i64().and_then(|n| i32::try_from(n).ok()),
        job_history_retention_days: |v: serde_json::Value| v.as_i64().and_then(|n| i32::try_from(n).ok()),
        notification_event_log_retention_days: |v: serde_json::Value| v.as_i64().and_then(|n| i32::try_from(n).ok()),
        default_download_engine: |v: serde_json::Value| v.as_str().map(String::from),
        // The request's Option<Value> treats JSON null like omission.
        default_extractor: |v: serde_json::Value| v.as_str().map(String::from),
        record_danmu: |v: serde_json::Value| v.as_bool(),
        // JSON-TEXT column: the object is stored verbatim and parsed at resolve
        // time, matching how the pipeline definitions are handled below. An object
        // is re-serialized; a string is taken as already-serialized JSON.
        danmu_statistics: |v: serde_json::Value| match v {
            serde_json::Value::Null => None,
            serde_json::Value::String(text) => Some(text),
            other => serde_json::to_string(&other).ok(),
        },
        proxy_config: |v: serde_json::Value| v.as_str().map(String::from),
        pipeline: |v: serde_json::Value| v.as_str().map(String::from),
        session_complete_pipeline: |v: serde_json::Value| v.as_str().map(String::from),
        paired_segment_pipeline: |v: serde_json::Value| v.as_str().map(String::from),
        auto_thumbnail: |v: serde_json::Value| v.as_bool(),
        pipeline_cpu_job_timeout_secs: |v: serde_json::Value| v.as_i64().map(|n| n.max(1)),
        pipeline_io_job_timeout_secs: |v: serde_json::Value| v.as_i64().map(|n| n.max(1)),
        pipeline_execute_timeout_secs: |v: serde_json::Value| v.as_i64().map(|n| n.max(1)),
        // Queue-wait freshness threshold. Floor at 0 (treats `<= 0`
        // as "always refetch on a wait"); upper bound is left to
        // sanity rather than a hard cap.
        queue_freshness_threshold_ms: |v: serde_json::Value| v.as_i64().map(|n| n.max(0)),
        // GPU health probe cadence (seconds). Clamped to >= 1 because each
        // probe is a `nvidia-smi` fork+exec (~50–200 ms); sub-second polling
        // would waste CPU. The UI hint discourages going below 30 s.
        gpu_health_probe_interval_secs: |v: serde_json::Value| v.as_i64().map(|n| n.max(1)),
        stream_proxy_allow_private_targets: |v: serde_json::Value| v.as_bool(),
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
        return Err(ApiError::from(e));
    }

    tracing::info!(
        updated_fields = %updated_fields_summary,
        "Global configuration updated successfully via API"
    );

    Ok(Json(map_global_config_to_response(config)?))
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
    State(state): State<ConfigRouteState>,
) -> ApiResult<Json<Vec<PlatformConfigResponse>>> {
    let config_service = &state.config_service;

    let configs = config_service
        .list_platform_configs()
        .await
        .map_err(ApiError::from)?;

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
    State(state): State<ConfigRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<PlatformConfigResponse>> {
    let config_service = &state.config_service;

    let config = config_service
        .get_platform_config(&id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(map_platform_config_to_response(config)))
}

/// Names of the overrides carried by `request`, for logging.
///
/// `replace_platform_config` must not record the body itself: `cookies`,
/// `platform_specific_config` (refresh/access tokens, SOOP username/password) and
/// `proxy_config` hold platform credentials.
fn platform_config_fields_present(request: &PlatformConfigResponse) -> Vec<&'static str> {
    macro_rules! present {
        ($($field:ident),+ $(,)?) => {
            [$((stringify!($field), request.$field.is_some())),+]
                .into_iter()
                .filter_map(|(name, is_set)| is_set.then_some(name))
                .collect()
        };
    }

    present!(
        fetch_delay_ms,
        download_delay_ms,
        record_danmu,
        danmu_statistics,
        cookies,
        platform_specific_config,
        proxy_config,
        output_folder,
        output_filename_template,
        download_engine,
        extractor,
        stream_selection_config,
        output_file_format,
        min_segment_size_bytes,
        max_download_duration_secs,
        max_part_size_bytes,
        download_retry_policy,
        pipeline,
        session_complete_pipeline,
        paired_segment_pipeline,
        offline_check_count,
        offline_check_delay_ms,
    )
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
    State(state): State<ConfigRouteState>,
    Path(id): Path<String>,
    Json(request): Json<PlatformConfigResponse>,
) -> ApiResult<Json<PlatformConfigResponse>> {
    tracing::info!(
        platform_id = %id,
        fields = %platform_config_fields_present(&request).join(", "),
        "Received request to replace platform configuration"
    );

    if request.id != id {
        return Err(ApiError::bad_request("Path ID does not match body ID"));
    }

    let config_service = &state.config_service;
    let stored = config_service
        .get_platform_config(&id)
        .await
        .map_err(ApiError::from)?;
    if request.name != stored.platform_name {
        return Err(ApiError::bad_request("Platform name cannot be changed"));
    }

    let offline_check_count = request
        .offline_check_count
        .map(|value| {
            i32::try_from(value).map_err(|_| {
                ApiError::bad_request(format!("offline_check_count must be at most {}", i32::MAX))
            })
        })
        .transpose()?;
    let offline_check_delay_ms =
        platform_i64("offline_check_delay_ms", request.offline_check_delay_ms)?;
    validate_offline_check_overrides(offline_check_count.map(i64::from), offline_check_delay_ms)?;

    // Build the full config model from request
    let config = PlatformConfigDbModel {
        id: request.id,
        platform_name: stored.platform_name,
        fetch_delay_ms: platform_i64("fetch_delay_ms", request.fetch_delay_ms)?,
        download_delay_ms: platform_i64("download_delay_ms", request.download_delay_ms)?,
        record_danmu: request.record_danmu,
        danmu_statistics: request.danmu_statistics,
        cookies: request.cookies,
        platform_specific_config: request.platform_specific_config,
        proxy_config: request.proxy_config,
        output_folder: request.output_folder,
        output_filename_template: request.output_filename_template,
        download_engine: request.download_engine,
        extractor: request.extractor,
        stream_selection_config: request.stream_selection_config,
        output_file_format: request.output_file_format,
        min_segment_size_bytes: platform_i64(
            "min_segment_size_bytes",
            request.min_segment_size_bytes,
        )?,
        max_download_duration_secs: platform_i64(
            "max_download_duration_secs",
            request.max_download_duration_secs,
        )?,
        max_part_size_bytes: platform_i64("max_part_size_bytes", request.max_part_size_bytes)?,
        download_retry_policy: request.download_retry_policy,
        pipeline: request.pipeline,
        session_complete_pipeline: request.session_complete_pipeline,
        paired_segment_pipeline: request.paired_segment_pipeline,
        offline_check_count,
        offline_check_delay_ms,
    };

    // Replace config
    if let Err(e) = config_service.update_platform_config(&config).await {
        tracing::error!(
            platform_id = %id,
            error = %e,
            "Failed to replace platform config"
        );
        return Err(ApiError::from(e));
    }

    tracing::info!(
        platform_id = %id,
        "Platform configuration replaced successfully"
    );

    Ok(Json(map_platform_config_to_response(config)))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::Json;
    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use super::{
        ConfigRouteState, map_platform_config_to_response, replace_platform_config,
        update_global_config, validate_global_config_request, validate_optional_retention_days,
        validate_retention_days,
    };
    use crate::api::models::{GlobalConfigResponse, UpdateGlobalConfigRequest};
    use crate::config::ConfigService;
    use crate::database;
    use crate::database::repositories::{SqlxConfigRepository, SqlxStreamerRepository};
    use crate::domain::value_objects::StreamerUrl;

    async fn config_state() -> (tempfile::TempDir, ConfigRouteState) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.db");
        let url = format!(
            "sqlite:{}?mode=rwc",
            path.to_string_lossy().replace('\\', "/")
        );
        let pool = database::init_pool(&url).await.unwrap();
        database::run_migrations(&pool).await.unwrap();
        let service = ConfigService::new(
            Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone())),
            Arc::new(SqlxStreamerRepository::new(pool.clone(), pool)),
        );
        (
            dir,
            ConfigRouteState {
                config_service: Arc::new(service),
            },
        )
    }

    fn global_request(value: Value) -> UpdateGlobalConfigRequest {
        serde_json::from_value(value).unwrap()
    }

    #[tokio::test]
    async fn platform_get_distinguishes_typed_not_found_from_database_error_text() {
        let pool = database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE platform_config (id TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        let state = ConfigRouteState {
            config_service: Arc::new(ConfigService::new(
                Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone())),
                Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone())),
            )),
        };
        let error = super::get_platform_config(State(state.clone()), Path("missing".to_string()))
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::NOT_FOUND);
        sqlx::query("DROP TABLE platform_config")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE VIEW platform_config AS SELECT * FROM \"not found\"")
            .execute(&pool)
            .await
            .unwrap();
        let error = super::get_platform_config(State(state), Path("missing".to_string()))
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error.message, "Database error occurred");
        pool.close().await;
    }

    #[test]
    fn global_numeric_patch_validates_types_and_boundaries() {
        let fields = [
            ("min_segment_size_bytes", 0, i64::MAX),
            ("max_download_duration_secs", 0, i64::MAX),
            ("max_part_size_bytes", 0, i64::MAX),
            ("max_concurrent_downloads", 1, i64::from(i32::MAX)),
            ("max_concurrent_uploads", 1, i64::from(i32::MAX)),
            ("max_concurrent_cpu_jobs", 0, i64::from(i32::MAX)),
            ("max_concurrent_io_jobs", 0, i64::from(i32::MAX)),
            ("streamer_check_delay_ms", 1, i64::MAX),
            ("offline_check_delay_ms", 1, i64::MAX),
            ("offline_check_count", 1, i64::from(i32::MAX)),
            ("job_history_retention_days", 0, i64::from(i32::MAX)),
            (
                "notification_event_log_retention_days",
                0,
                i64::from(i32::MAX),
            ),
            ("pipeline_cpu_job_timeout_secs", i64::MIN, i64::MAX),
            ("pipeline_io_job_timeout_secs", i64::MIN, i64::MAX),
            ("pipeline_execute_timeout_secs", i64::MIN, i64::MAX),
            ("queue_freshness_threshold_ms", i64::MIN, i64::MAX),
            ("gpu_health_probe_interval_secs", i64::MIN, i64::MAX),
        ];
        for (field, min, max) in fields {
            let mut invalid = vec![
                json!(1.5),
                json!("1"),
                json!(true),
                json!([]),
                json!({}),
                json!(u64::MAX),
                json!(i128::from(max) + 1),
            ];
            if let Some(below_min) = min.checked_sub(1) {
                invalid.push(json!(below_min));
            }
            for value in invalid {
                let request = global_request(json!({field: value}));
                let error = validate_global_config_request(&request).expect_err(field);
                assert_eq!(error.status, StatusCode::BAD_REQUEST, "{field}");
                assert!(error.message.contains(field), "{}", error.message);
            }
            for value in [json!(min), json!(max), Value::Null] {
                let representable = field != "gpu_health_probe_interval_secs"
                    || value.as_i64().is_none_or(|seconds| {
                        std::time::Instant::now()
                            .checked_add(std::time::Duration::from_secs(
                                seconds.max(1).unsigned_abs(),
                            ))
                            .is_some()
                    });
                let result = validate_global_config_request(&global_request(json!({field: value})));
                assert_eq!(result.is_ok(), representable, "{field}: {result:?}");
            }
        }
    }

    #[tokio::test]
    async fn global_patch_checks_gpu_timer_range_before_mutation() {
        let (_dir, state) = config_state().await;
        let before = state.config_service.get_global_config().await.unwrap();
        let representable = std::time::Instant::now()
            .checked_add(std::time::Duration::from_secs(i64::MAX.unsigned_abs()))
            .is_some();
        let result = update_global_config(
            State(state.clone()),
            Json(global_request(json!({
                "output_folder": "/changed",
                "gpu_health_probe_interval_secs": i64::MAX,
            }))),
        )
        .await;
        let after = state.config_service.get_global_config().await.unwrap();
        if representable {
            let Json(response) = result.unwrap();
            assert_eq!(response.output_folder, "/changed");
            assert_eq!(after.output_folder, "/changed");
            assert_eq!(after.gpu_health_probe_interval_secs, i64::MAX);
        } else {
            let error = result.expect_err("unrepresentable GPU interval must be rejected");
            assert_eq!(error.status, StatusCode::BAD_REQUEST);
            assert!(error.message.contains("gpu_health_probe_interval_secs"));
            assert_eq!(
                serde_json::to_value(&after).unwrap(),
                serde_json::to_value(&before).unwrap()
            );
        }
    }

    #[test]
    fn global_patch_validates_string_and_boolean_fields() {
        for field in [
            "output_folder",
            "output_filename_template",
            "output_file_format",
            "default_download_engine",
            "default_extractor",
            "proxy_config",
            "pipeline",
            "session_complete_pipeline",
            "paired_segment_pipeline",
        ] {
            let error = validate_global_config_request(&global_request(json!({field: {}})))
                .expect_err(field);
            assert_eq!(error.status, StatusCode::BAD_REQUEST);
            assert!(error.message.contains(field));
            validate_global_config_request(&global_request(json!({field: "value"}))).unwrap();
            validate_global_config_request(&global_request(json!({field: null}))).unwrap();
        }
        for field in [
            "record_danmu",
            "auto_thumbnail",
            "stream_proxy_allow_private_targets",
        ] {
            let error = validate_global_config_request(&global_request(json!({field: "true"})))
                .expect_err(field);
            assert_eq!(error.status, StatusCode::BAD_REQUEST);
            assert!(error.message.contains(field));
            for value in [json!(true), json!(false), Value::Null] {
                validate_global_config_request(&global_request(json!({field: value}))).unwrap();
            }
        }
    }

    #[tokio::test]
    async fn invalid_global_patch_leaves_all_stored_fields_unchanged() {
        let (_dir, state) = config_state().await;
        let before = state.config_service.get_global_config().await.unwrap();
        let mut events = state.config_service.subscribe();
        for invalid in [
            json!({"max_concurrent_downloads": -1}),
            json!({"max_concurrent_uploads": i64::from(i32::MAX) + 1}),
            json!({"offline_check_delay_ms": u64::MAX}),
            json!({"offline_check_count": 1.5}),
            json!({"max_part_size_bytes": "12"}),
            json!({"record_danmu": "true"}),
            json!({"pipeline": {"steps": []}}),
        ] {
            let mut patch = invalid;
            patch["output_folder"] = json!("/changed");
            let error = update_global_config(State(state.clone()), Json(global_request(patch)))
                .await
                .expect_err("mixed invalid patch must fail");
            assert_eq!(error.status, StatusCode::BAD_REQUEST);
            let after = state.config_service.get_global_config().await.unwrap();
            assert_eq!(
                serde_json::to_value(&after).unwrap(),
                serde_json::to_value(&before).unwrap()
            );
            assert!(matches!(
                events.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ));
        }
    }

    #[tokio::test]
    async fn global_patch_preserves_null_omission_sentinels_and_clamps() {
        let (_dir, state) = config_state().await;
        let Json(response) = update_global_config(
            State(state.clone()),
            Json(global_request(json!({
                "default_extractor": "streamlink"
            }))),
        )
        .await
        .unwrap();
        assert_eq!(response.default_extractor.as_deref(), Some("streamlink"));
        let before = state.config_service.get_global_config().await.unwrap();
        let request = global_request(json!({
            "output_folder": null,
            "default_extractor": null,
            "min_segment_size_bytes": 0,
            "max_download_duration_secs": 0,
            "max_part_size_bytes": 0,
            "max_concurrent_downloads": i32::MAX,
            "max_concurrent_uploads": 1,
            "max_concurrent_cpu_jobs": 0,
            "max_concurrent_io_jobs": 0,
            "job_history_retention_days": 0,
            "notification_event_log_retention_days": i32::MAX,
            "offline_check_count": 1,
            "streamer_check_delay_ms": 1,
            "offline_check_delay_ms": 1,
            "pipeline_cpu_job_timeout_secs": i64::MIN,
            "pipeline_io_job_timeout_secs": -1,
            "pipeline_execute_timeout_secs": 0,
            "gpu_health_probe_interval_secs": -1,
            "queue_freshness_threshold_ms": -1
        }));
        let Json(response) = update_global_config(State(state.clone()), Json(request))
            .await
            .unwrap();
        assert_eq!(response.max_concurrent_cpu_jobs, 0);
        let after = state.config_service.get_global_config().await.unwrap();
        assert_eq!(after.output_folder, before.output_folder);
        assert_eq!(
            after.output_filename_template,
            before.output_filename_template
        );
        assert_eq!(after.default_extractor, before.default_extractor);
        assert_eq!(
            (
                after.min_segment_size_bytes,
                after.max_download_duration_secs,
                after.max_part_size_bytes
            ),
            (0, 0, 0)
        );
        assert_eq!(
            (after.max_concurrent_downloads, after.max_concurrent_uploads),
            (i32::MAX, 1)
        );
        assert_eq!(
            (after.max_concurrent_cpu_jobs, after.max_concurrent_io_jobs),
            (0, 0)
        );
        assert_eq!(
            (
                after.job_history_retention_days,
                after.notification_event_log_retention_days
            ),
            (0, i32::MAX)
        );
        assert_eq!(
            (
                after.offline_check_count,
                after.streamer_check_delay_ms,
                after.offline_check_delay_ms
            ),
            (1, 1, 1)
        );
        assert_eq!(
            (
                after.pipeline_cpu_job_timeout_secs,
                after.pipeline_io_job_timeout_secs,
                after.pipeline_execute_timeout_secs
            ),
            (1, 1, 1)
        );
        assert_eq!(after.gpu_health_probe_interval_secs, 1);
        assert_eq!(after.queue_freshness_threshold_ms, 0);

        let Json(response) = update_global_config(
            State(state.clone()),
            Json(global_request(json!({
                "pipeline_cpu_job_timeout_secs": 12,
                "pipeline_io_job_timeout_secs": 13,
                "pipeline_execute_timeout_secs": 14,
                "gpu_health_probe_interval_secs": 15,
                "queue_freshness_threshold_ms": 16,
                "danmu_statistics": {"enabled": true}
            }))),
        )
        .await
        .unwrap();
        assert_eq!(response.pipeline_cpu_job_timeout_secs, 12);
        let after = state.config_service.get_global_config().await.unwrap();
        assert_eq!(
            (
                after.pipeline_cpu_job_timeout_secs,
                after.pipeline_io_job_timeout_secs,
                after.pipeline_execute_timeout_secs
            ),
            (12, 13, 14)
        );
        assert_eq!(after.gpu_health_probe_interval_secs, 15);
        assert_eq!(after.queue_freshness_threshold_ms, 16);
        assert_eq!(
            serde_json::from_str::<Value>(after.danmu_statistics.as_deref().unwrap()).unwrap(),
            json!({"enabled": true})
        );
    }

    #[tokio::test]
    async fn platform_replacement_preserves_exact_identity_and_resolvability() {
        let (_dir, state) = config_state().await;
        let before = state
            .config_service
            .get_platform_config_by_name("bilibili")
            .await
            .unwrap();
        let original = map_platform_config_to_response(before.clone());
        for name in ["renamed", "Bilibili"] {
            let mut request = original.clone();
            request.name = name.to_string();
            request.fetch_delay_ms = Some(1234);
            let error = replace_platform_config(
                State(state.clone()),
                Path(before.id.clone()),
                Json(request),
            )
            .await
            .expect_err("platform identity must not change");
            assert_eq!(error.status, StatusCode::BAD_REQUEST);
            let stored = state
                .config_service
                .get_platform_config(&before.id)
                .await
                .unwrap();
            assert_eq!(
                serde_json::to_value(&stored).unwrap(),
                serde_json::to_value(&before).unwrap()
            );
        }
        let mut request = original;
        request.fetch_delay_ms = Some(1234);
        let Json(response) =
            replace_platform_config(State(state.clone()), Path(before.id.clone()), Json(request))
                .await
                .unwrap();
        assert_eq!(response.name, before.platform_name);
        let url = StreamerUrl::new("https://live.bilibili.com/12345").unwrap();
        let stored = state
            .config_service
            .get_platform_config_by_name(&url.platform().unwrap().to_lowercase())
            .await
            .unwrap();
        assert_eq!(stored.id, before.id);
        assert_eq!(stored.fetch_delay_ms, Some(1234));
    }

    #[tokio::test]
    async fn platform_replacement_rejects_missing_rows_and_numeric_overflow() {
        let (_dir, state) = config_state().await;
        let before = state
            .config_service
            .get_platform_config_by_name("bilibili")
            .await
            .unwrap();
        let original = map_platform_config_to_response(before.clone());
        let mut missing = original.clone();
        missing.id = "missing".to_string();
        let error = replace_platform_config(
            State(state.clone()),
            Path(missing.id.clone()),
            Json(missing),
        )
        .await
        .expect_err("missing platform must fail");
        assert_eq!(error.status, StatusCode::NOT_FOUND);

        for field in [
            "fetch_delay_ms",
            "download_delay_ms",
            "min_segment_size_bytes",
            "max_download_duration_secs",
            "max_part_size_bytes",
            "offline_check_delay_ms",
            "offline_check_count",
        ] {
            let mut value = serde_json::to_value(&original).unwrap();
            value[field] = if field == "offline_check_count" {
                json!(u32::MAX)
            } else {
                json!(u64::MAX)
            };
            value["output_folder"] = json!("/changed");
            let error = replace_platform_config(
                State(state.clone()),
                Path(before.id.clone()),
                Json(serde_json::from_value(value).unwrap()),
            )
            .await
            .expect_err("overflow must not be stored");
            assert_eq!(error.status, StatusCode::BAD_REQUEST);
            assert!(error.message.contains(field));
            let stored = state
                .config_service
                .get_platform_config(&before.id)
                .await
                .unwrap();
            assert_eq!(
                serde_json::to_value(&stored).unwrap(),
                serde_json::to_value(&before).unwrap()
            );
        }
    }

    #[test]
    fn retention_validation_accepts_zero_and_positive_integers() {
        assert!(validate_retention_days("retention", 0).is_ok());
        assert!(validate_retention_days("retention", 30).is_ok());
    }

    #[test]
    fn retention_validation_rejects_negative_or_non_integer_values() {
        let error = validate_retention_days("retention", -1)
            .expect_err("negative retention must be rejected");
        assert_eq!(error.status, StatusCode::BAD_REQUEST);

        let value = serde_json::json!(1.5);
        let error = validate_optional_retention_days("retention", Some(&value))
            .expect_err("fractional retention must be rejected");
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

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
            danmu_statistics: None,
            max_concurrent_downloads: 6,
            max_concurrent_uploads: 3,
            streamer_check_delay_ms: 60000,
            proxy_config: None,
            offline_check_delay_ms: 20000,
            offline_check_count: 3,
            default_download_engine: "mesio".to_string(),
            default_extractor: None,
            max_concurrent_cpu_jobs: 0,
            max_concurrent_io_jobs: 8,
            job_history_retention_days: 30,
            notification_event_log_retention_days: 30,
            pipeline: None,
            session_complete_pipeline: None,
            paired_segment_pipeline: None,
            log_filter_directive: "rust_srec=info".to_string(),
            auto_thumbnail: true,

            pipeline_cpu_job_timeout_secs: 3600,
            pipeline_io_job_timeout_secs: 3600,
            pipeline_execute_timeout_secs: 3600,
            queue_freshness_threshold_ms: 60_000,
            gpu_health_probe_interval_secs: 30,
            stream_proxy_allow_private_targets: false,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("downloads"));
        assert!(json.contains("mesio"));
    }
}
