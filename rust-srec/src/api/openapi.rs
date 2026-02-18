//! OpenAPI documentation configuration.
//!
//! This module configures OpenAPI 3.0 specification generation using `utoipa`
//! and serves Swagger UI for interactive API exploration.

use utoipa::OpenApi;

use crate::api::models::{
    ComponentHealth, CreateFilterRequest, CreateStreamerRequest, CreateTemplateRequest,
    DanmuRatePoint, DanmuTopTalker, DanmuWordFrequency, ExtractMetadataRequest,
    ExtractMetadataResponse, FilterResponse, GlobalConfigResponse, HealthResponse, JobResponse,
    PaginatedResponse, ParseUrlRequest, ParseUrlResponse, PipelineStatsResponse,
    PlatformConfigResponse, ResolveUrlRequest, ResolveUrlResponse, SessionDanmuStatisticsResponse,
    SessionResponse, StreamerResponse, TemplateResponse, UpdateFilterRequest,
    UpdateGlobalConfigRequest, UpdatePriorityRequest, UpdateStreamerRequest, UpdateTemplateRequest,
};
use crate::api::routes::auth::{
    ChangePasswordRequest, LoginRequest, LoginResponse, LogoutRequest, RefreshRequest,
};
use crate::api::routes::credentials::{
    CredentialRefreshResponse, CredentialSaveScope, CredentialSourceResponse,
    QrGenerateApiResponse, QrPollApiResponse, QrPollRequest,
};
use crate::api::routes::engines::{CreateEngineRequest, EngineTestResponse, UpdateEngineRequest};
use crate::api::routes::export_import::{
    ConfigExport, ImportMode, ImportRequest, ImportResult, ImportStats,
};
use crate::api::routes::job::{
    ClonePresetRequest, CreatePresetRequest, PresetListResponse, UpdatePresetRequest,
};
use crate::api::routes::logging::{
    ArchiveTokenResponse, LogEntriesResponse, LogEntry, LogFileInfo, LogFilesResponse,
};
use crate::api::routes::logging::{LoggingConfigResponse, ModuleInfo, UpdateLogFilterRequest};
use crate::api::routes::notifications::{
    CreateChannelRequest, UpdateChannelRequest, UpdateSubscriptionsRequest,
};
use crate::api::routes::pipeline::{
    CreatePipelinePresetRequest, CreatePipelineRequest, CreatePipelineResponse, DagCancelResponse,
    DagGraphResponse, DagListResponse, DagRetryResponse, DagStatsResponse, DagStatusResponse,
    PipelinePresetListResponse, PipelinePresetResponse, PresetPreviewResponse,
    UpdatePipelinePresetRequest, ValidateDagRequest, ValidateDagResponse,
};

/// Liveness check response.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct LivenessResponse {
    /// Status indicator (always "alive" if responding)
    pub status: String,
    /// Server uptime in seconds
    pub uptime_secs: u64,
}

/// Generic message response for operations that return only a status message.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct MessageResponse {
    /// Status or result message
    pub message: String,
}

/// OpenAPI documentation for the rust-srec API.
///
/// This struct aggregates all documented endpoints and schemas.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "rust-srec API",
        version = env!("CARGO_PKG_VERSION"),
        description = "REST API for the rust-srec streaming recorder. Provides endpoints for managing streamers, recording sessions, pipeline jobs, and system configuration.",
        license(name = "MIT OR Apache-2.0"),
        contact(name = "rust-srec", url = "https://github.com/hua0512/rust-srec")
    ),
    servers(
        (url = "http://localhost:12555", description = "Local development server")
    ),
    tags(
        (name = "health", description = "Health check endpoints for monitoring and orchestration"),
        (name = "auth", description = "Authentication endpoints for login, logout, and token management"),
        (name = "streamers", description = "Streamer management endpoints"),
        (name = "config", description = "Configuration management endpoints"),
        (name = "sessions", description = "Recording session endpoints"),
        (name = "templates", description = "Template configuration endpoints"),
        (name = "pipeline", description = "Pipeline job management endpoints"),
        (name = "filters", description = "Streamer filter management endpoints"),
        (name = "parse", description = "URL parsing and stream detection endpoints"),
        (name = "logging", description = "Logging configuration endpoints"),
        (name = "media", description = "Media content delivery endpoints"),
        (name = "engines", description = "Download engine configuration endpoints"),
        (name = "notifications", description = "Notification channel management endpoints"),
        (name = "job", description = "Job preset management endpoints"),
        (name = "export_import", description = "Configuration backup and restore endpoints")
        ,
        (name = "credentials", description = "Credential refresh and provenance endpoints")
    ),
    paths(
        // Health endpoints
        crate::api::routes::health::health_check,
        crate::api::routes::health::readiness_check,
        crate::api::routes::health::liveness_check,
        // Auth endpoints
        crate::api::routes::auth::login,
        crate::api::routes::auth::refresh,
        crate::api::routes::auth::logout,
        crate::api::routes::auth::logout_all,
        crate::api::routes::auth::change_password,
        crate::api::routes::auth::list_sessions,
        // Streamer endpoints
        crate::api::routes::streamers::create_streamer,
        crate::api::routes::streamers::list_streamers,
        crate::api::routes::streamers::get_streamer,
        crate::api::routes::streamers::update_streamer,
        crate::api::routes::streamers::delete_streamer,
        crate::api::routes::streamers::clear_error,
        crate::api::routes::streamers::update_priority,
        crate::api::routes::streamers::extract_metadata,
        // Config endpoints
        crate::api::routes::config::get_global_config,
        crate::api::routes::config::update_global_config,
        crate::api::routes::config::list_platform_configs,
        crate::api::routes::config::get_platform_config,
        crate::api::routes::config::replace_platform_config,
        // Session endpoints
        crate::api::routes::sessions::list_sessions,
        crate::api::routes::sessions::get_session,
        crate::api::routes::sessions::get_session_danmu_statistics,
        crate::api::routes::sessions::delete_session,
        crate::api::routes::sessions::delete_sessions_batch,
        // Template endpoints
        crate::api::routes::templates::create_template,
        crate::api::routes::templates::list_templates,
        crate::api::routes::templates::get_template,
        crate::api::routes::templates::update_template,
        crate::api::routes::templates::delete_template,
        // Pipeline endpoints
        crate::api::routes::pipeline::list_jobs,
        crate::api::routes::pipeline::list_jobs_page,
        crate::api::routes::pipeline::get_job,
        crate::api::routes::pipeline::list_job_logs,
        crate::api::routes::pipeline::get_job_progress,
        crate::api::routes::pipeline::retry_job,
        crate::api::routes::pipeline::cancel_job,
        crate::api::routes::pipeline::cancel_pipeline,
        crate::api::routes::pipeline::get_stats,
        crate::api::routes::pipeline::list_outputs,
        crate::api::routes::pipeline::create_pipeline,
        crate::api::routes::pipeline::validate_dag,
        // Pipeline preset endpoints
        crate::api::routes::pipeline::list_pipeline_presets,
        crate::api::routes::pipeline::get_pipeline_preset_by_id,
        crate::api::routes::pipeline::create_pipeline_preset,
        crate::api::routes::pipeline::update_pipeline_preset,
        crate::api::routes::pipeline::delete_pipeline_preset,
        crate::api::routes::pipeline::preview_pipeline_preset,
        // DAG endpoints
        crate::api::routes::pipeline::list_dags,
        crate::api::routes::pipeline::retry_all_failed_dags,
        crate::api::routes::pipeline::get_dag_status,
        crate::api::routes::pipeline::get_dag_graph,
        crate::api::routes::pipeline::get_dag_stats,
        crate::api::routes::pipeline::retry_dag,
        crate::api::routes::pipeline::cancel_dag,
        crate::api::routes::pipeline::delete_dag,
        // Filter endpoints
        crate::api::routes::filters::list_filters,
        crate::api::routes::filters::create_filter,
        crate::api::routes::filters::get_filter,
        crate::api::routes::filters::update_filter,
        crate::api::routes::filters::delete_filter,
        // Parse endpoints
        crate::api::routes::parse::parse_url,
        crate::api::routes::parse::parse_url_batch,
        crate::api::routes::parse::resolve_url,
        // Logging endpoints
        crate::api::routes::logging::get_logging_config,
        crate::api::routes::logging::update_logging_config,
        crate::api::routes::logging::list_log_files,
        crate::api::routes::logging::list_log_entries,
        crate::api::routes::logging::get_archive_token,
        crate::api::routes::logging::download_logs_archive,
        // Media endpoints
        crate::api::routes::media::get_media_content,
        // Engine endpoints
        crate::api::routes::engines::list_engines,
        crate::api::routes::engines::get_engine,
        crate::api::routes::engines::create_engine,
        crate::api::routes::engines::update_engine,
        crate::api::routes::engines::delete_engine,
        crate::api::routes::engines::test_engine,
        // Notification endpoints
        crate::api::routes::notifications::list_event_types,
        crate::api::routes::notifications::list_events,
        crate::api::routes::notifications::list_instances,
        crate::api::routes::notifications::list_channels,
        crate::api::routes::notifications::get_channel,
        crate::api::routes::notifications::create_channel,
        crate::api::routes::notifications::update_channel,
        crate::api::routes::notifications::delete_channel,
        crate::api::routes::notifications::get_subscriptions,
        crate::api::routes::notifications::update_subscriptions,
        crate::api::routes::notifications::test_channel,
        // Job preset endpoints
        crate::api::routes::job::list_presets,
        crate::api::routes::job::get_preset,
        crate::api::routes::job::create_preset,
        crate::api::routes::job::update_preset,
        crate::api::routes::job::delete_preset,
        crate::api::routes::job::clone_preset,
        // Export/Import endpoints
        crate::api::routes::export_import::export_config,
        crate::api::routes::export_import::import_config,
        // Credentials endpoints
        crate::api::routes::credentials::get_streamer_credential_source,
        crate::api::routes::credentials::refresh_streamer_credentials,
        crate::api::routes::credentials::get_platform_credential_source,
        crate::api::routes::credentials::refresh_platform_credentials,
        crate::api::routes::credentials::get_template_credential_source,
        crate::api::routes::credentials::refresh_template_credentials,
        crate::api::routes::credentials::bilibili_qr_generate,
        crate::api::routes::credentials::bilibili_qr_poll,
    ),
    components(
        schemas(
            // Health schemas
            HealthResponse,
            ComponentHealth,
            LivenessResponse,
            // Auth schemas
            LoginRequest,
            LoginResponse,
            RefreshRequest,
            LogoutRequest,
            ChangePasswordRequest,
            MessageResponse,
            crate::api::auth_service::SessionInfo,
            // Error schema
            crate::api::error::ApiErrorResponse,
            // Streamer schemas
            CreateStreamerRequest,
            UpdateStreamerRequest,
            UpdatePriorityRequest,
            StreamerResponse,
            PaginatedResponse<StreamerResponse>,
            ExtractMetadataRequest,
            ExtractMetadataResponse,
            // Config schemas
            GlobalConfigResponse,
            UpdateGlobalConfigRequest,
            PlatformConfigResponse,
            // Session schemas
            SessionResponse,
            SessionDanmuStatisticsResponse,
            DanmuRatePoint,
            DanmuTopTalker,
            DanmuWordFrequency,
            PaginatedResponse<SessionResponse>,
            crate::api::routes::sessions::BatchDeleteRequest,
            crate::api::routes::sessions::BatchDeleteResponse,
            // Template schemas
            CreateTemplateRequest,
            UpdateTemplateRequest,
            TemplateResponse,
            PaginatedResponse<TemplateResponse>,
            // Pipeline schemas
            JobResponse,
            PaginatedResponse<JobResponse>,
            PipelineStatsResponse,
            // Filter schemas
            CreateFilterRequest,
            UpdateFilterRequest,
            FilterResponse,
            // Parse schemas
            ParseUrlRequest,
            ParseUrlResponse,
            ResolveUrlRequest,
            ResolveUrlResponse,
            // Logging schemas
            UpdateLogFilterRequest,
            LoggingConfigResponse,
            ModuleInfo,
            LogFileInfo,
            LogFilesResponse,
            ArchiveTokenResponse,
            LogEntry,
            LogEntriesResponse,
            // Engine schemas
            CreateEngineRequest,
            UpdateEngineRequest,
            EngineTestResponse,
            crate::database::models::EngineConfigurationDbModel,
            crate::database::models::EngineType,
            // Notification schemas
            CreateChannelRequest,
            UpdateChannelRequest,
            UpdateSubscriptionsRequest,
            crate::api::routes::notifications::ListEventsQuery,
            crate::database::models::notification::NotificationChannelDbModel,
            crate::database::models::notification::ChannelType,
            crate::database::models::notification::NotificationEventLogDbModel,
            crate::notification::events::NotificationEventTypeInfo,
            crate::notification::service::NotificationChannelInstance,
            // Job preset schemas
            CreatePresetRequest,
            UpdatePresetRequest,
            ClonePresetRequest,
            PresetListResponse,
            crate::database::models::JobPreset,
            // Export/Import schemas
            ConfigExport,
            ImportRequest,
            ImportMode,
            ImportResult,
            ImportStats,
            // Credentials schemas
            CredentialSourceResponse,
            CredentialRefreshResponse,
            QrGenerateApiResponse,
            CredentialSaveScope,
            QrPollRequest,
            QrPollApiResponse,
            // Pipeline DAG schemas
            CreatePipelineRequest,
            CreatePipelineResponse,
            CreatePipelinePresetRequest,
            UpdatePipelinePresetRequest,
            PipelinePresetListResponse,
            PipelinePresetResponse,
            PresetPreviewResponse,
            DagStatusResponse,
            DagGraphResponse,
            DagListResponse,
            DagRetryResponse,
            DagCancelResponse,
            DagStatsResponse,
            ValidateDagRequest,
            ValidateDagResponse,
            crate::database::models::job::DagPipelineDefinition,
            crate::database::models::job::DagStep,
            crate::database::models::job::PipelineStep,
        )
    ),
    security(
        ("bearer_auth" = [])
    ),
    modifiers(&SecurityAddon)
)]
pub struct ApiDoc;

/// Security scheme addon for Bearer JWT authentication.
struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer_auth",
                utoipa::openapi::security::SecurityScheme::Http(
                    utoipa::openapi::security::HttpBuilder::new()
                        .scheme(utoipa::openapi::security::HttpAuthScheme::Bearer)
                        .bearer_format("JWT")
                        .build(),
                ),
            );
        }
    }
}
