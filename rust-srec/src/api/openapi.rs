//! OpenAPI documentation configuration.
//!
//! This module configures OpenAPI 3.0 specification generation using `utoipa`
//! and serves Swagger UI for interactive API exploration.

use utoipa::OpenApi;

#[cfg(test)]
mod coverage;

use crate::api::backup::{ConfigExport, ImportMode, ImportRequest, ImportResult, ImportStats};
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
    ApiKeyResponse, ChangePasswordRequest, CreateApiKeyRequest, CreateApiKeyResponse, LoginRequest,
    LoginResponse, LogoutRequest, RefreshRequest,
};
use crate::api::routes::baidupcs::{
    BaiduPcsLoginRequest, BaiduPcsLoginResponse, BaiduPcsLogoutResponse, BaiduPcsStatusResponse,
    BaiduPcsToolRequest,
};
use crate::api::routes::engines::{CreateEngineRequest, EngineTestResponse, UpdateEngineRequest};
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
use crate::api::routes::pipeline::interrupted::{
    BatchDeleteInterruptedOutputsRequest, DeleteInterruptedOutputResponse,
    InterruptedOutputListResponse, InterruptedOutputResponse,
};
use crate::api::routes::pipeline::{
    BatchDagAction, BatchDagItemResult, BatchDagRequest, BatchDagResponse,
    BatchDeleteOutputsRequest, BatchDeleteOutputsResponse, CreatePipelinePresetRequest,
    CreatePipelineRequest, CreatePipelineResponse, DagCancelResponse, DagGraphResponse,
    DagListResponse, DagListStatsResponse, DagRetryResponse, DagStatsResponse, DagStatusResponse,
    DeleteOutputResponse, PipelinePresetListResponse, PipelinePresetResponse,
    PresetPreviewResponse, UpdatePipelinePresetRequest, ValidateDagRequest, ValidateDagResponse,
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
        (name = "credentials", description = "Account profile, selection and sign-in endpoints"),
        (name = "proxies", description = "Saved proxies that scope and account routes name"),
        (name = "tools", description = "External tool integration endpoints (BaiduPCS-Go)")
    ),
    paths(
        crate::api::routes::downloads::request_manual_split,
        // Health endpoints
        crate::api::routes::health::health_check,
        crate::api::routes::health::readiness_check,
        crate::api::routes::health::liveness_check,
        crate::api::routes::health::idle_check,
        // Auth endpoints
        crate::api::routes::auth::login,
        crate::api::routes::auth::refresh,
        crate::api::routes::auth::logout,
        crate::api::routes::auth::logout_all,
        crate::api::routes::auth::change_password,
        crate::api::routes::auth::list_sessions,
        crate::api::routes::auth::list_api_keys,
        crate::api::routes::auth::create_api_key,
        crate::api::routes::auth::revoke_api_key,
        // Streamer endpoints
        crate::api::routes::streamers::create_streamer,
        crate::api::routes::streamers::list_streamers,
        crate::api::routes::streamers::batch_streamers,
        crate::api::routes::streamers::get_streamer,
        crate::api::routes::streamers::update_streamer,
        crate::api::routes::streamers::delete_streamer,
        crate::api::routes::streamers::clear_error,
        crate::api::routes::streamers::update_priority,
        crate::api::routes::streamers::extract_metadata,
        crate::api::routes::streamers::get_check_history,
        // Config endpoints
        crate::api::routes::config::get_global_config,
        crate::api::routes::config::update_global_config,
        crate::api::routes::config::list_platform_configs,
        crate::api::routes::config::get_platform_config,
        crate::api::routes::config::replace_platform_config,
        crate::api::routes::config::get_bigo_notifications,
        crate::api::routes::config::reconnect_bigo_notifications,
        // Session endpoints
        crate::api::routes::sessions::list_sessions,
        crate::api::routes::sessions::get_session,
        crate::api::routes::sessions::get_session_danmu_statistics,
        crate::api::routes::sessions::list_session_segments,
        crate::api::routes::sessions::delete_session,
        crate::api::routes::sessions::delete_sessions_batch,
        // Template endpoints
        crate::api::routes::templates::create_template,
        crate::api::routes::templates::clone_template,
        crate::api::routes::templates::list_templates,
        crate::api::routes::templates::get_template,
        crate::api::routes::templates::update_template,
        crate::api::routes::templates::delete_template,
        // Pipeline endpoints
        crate::api::routes::pipeline::jobs::list_jobs,
        crate::api::routes::pipeline::jobs::list_jobs_page,
        crate::api::routes::pipeline::jobs::get_job,
        crate::api::routes::pipeline::jobs::list_job_logs,
        crate::api::routes::pipeline::jobs::get_job_progress,
        crate::api::routes::pipeline::jobs::list_job_uploads,
        crate::api::routes::pipeline::jobs::retry_job,
        crate::api::routes::pipeline::jobs::cancel_job,
        crate::api::routes::pipeline::jobs::delete_job,
        crate::api::routes::pipeline::jobs::cancel_pipeline,
        crate::api::routes::pipeline::jobs::get_stats,
        crate::api::routes::pipeline::jobs::list_stalled_operations,
        crate::api::routes::pipeline::jobs::list_outputs,
        crate::api::routes::pipeline::jobs::summarize_outputs,
        crate::api::routes::pipeline::jobs::create_pipeline,
        crate::api::routes::pipeline::dag::validate_dag,
        // Pipeline preset endpoints
        crate::api::routes::pipeline::presets::list_pipeline_presets,
        crate::api::routes::pipeline::presets::get_pipeline_preset_by_id,
        crate::api::routes::pipeline::presets::create_pipeline_preset,
        crate::api::routes::pipeline::presets::update_pipeline_preset,
        crate::api::routes::pipeline::presets::delete_pipeline_preset,
        crate::api::routes::pipeline::presets::preview_pipeline_preset,
        // DAG endpoints
        crate::api::routes::pipeline::dag::list_dags,
        crate::api::routes::pipeline::dag::get_dag_list_stats,
        crate::api::routes::pipeline::dag::retry_all_failed_dags,
        crate::api::routes::pipeline::dag::batch_dags,
        crate::api::routes::pipeline::jobs::delete_output,
        crate::api::routes::pipeline::jobs::batch_delete_outputs,
        crate::api::routes::pipeline::interrupted::list_interrupted_outputs,
        crate::api::routes::pipeline::interrupted::delete_interrupted_output,
        crate::api::routes::pipeline::interrupted::batch_delete_interrupted_outputs,
        crate::api::routes::pipeline::dag::get_dag_status,
        crate::api::routes::pipeline::dag::get_dag_graph,
        crate::api::routes::pipeline::dag::get_dag_stats,
        crate::api::routes::pipeline::dag::retry_dag,
        crate::api::routes::pipeline::dag::cancel_dag,
        crate::api::routes::pipeline::dag::delete_dag,
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
        crate::api::routes::parse::renew_playback,
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
        crate::api::routes::notifications::get_web_push_public_key,
        crate::api::routes::notifications::list_web_push_subscriptions,
        crate::api::routes::notifications::subscribe_web_push,
        crate::api::routes::notifications::unsubscribe_web_push,
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
        crate::api::routes::credential_profiles::list_profiles,
        crate::api::routes::credential_profiles::get_capabilities,
        crate::api::routes::credential_profiles::list_attention,
        crate::api::routes::credential_profiles::get_selection,
        crate::api::routes::credential_profiles::get_profile,
        crate::api::routes::credential_profiles::create_profile,
        crate::api::routes::credential_profiles::update_profile,
        crate::api::routes::credential_profiles::delete_profile,
        crate::api::routes::credential_profiles::validate_profile,
        crate::api::routes::credential_profiles::refresh_profile,
        crate::api::routes::credential_profiles::generate_login,
        crate::api::routes::credential_profiles::poll_login,
        // Proxy endpoints
        crate::api::routes::proxies::list_proxies,
        crate::api::routes::proxies::get_proxy,
        crate::api::routes::proxies::create_proxy,
        crate::api::routes::proxies::update_proxy,
        crate::api::routes::proxies::delete_proxy,
        crate::api::routes::proxies::system_proxy,
        crate::api::routes::proxies::effective_route,
        crate::api::routes::proxies::test_proxy,
        // BaiduPCS-Go tool endpoints
        crate::api::routes::baidupcs::baidupcs_status,
        crate::api::routes::baidupcs::baidupcs_login,
        crate::api::routes::baidupcs::baidupcs_logout,
    ),
    components(
        schemas(
            // Health schemas
            HealthResponse,
            ComponentHealth,
            crate::api::schemas::DiskUsage,
            LivenessResponse,
            crate::api::routes::health::IdleResponse,
            // Auth schemas
            LoginRequest,
            LoginResponse,
            RefreshRequest,
            LogoutRequest,
            ChangePasswordRequest,
            MessageResponse,
            crate::api::auth_service::SessionInfo,
            CreateApiKeyRequest,
            CreateApiKeyResponse,
            ApiKeyResponse,
            crate::api::auth_store::ApiKeyAccessLevel,
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
            crate::api::models::StreamerCheckHistoryEntry,
            crate::api::models::StreamerCheckHistoryResponse,
            // Config schemas
            GlobalConfigResponse,
            UpdateGlobalConfigRequest,
            PlatformConfigResponse,
            // Session schemas
            SessionResponse,
            crate::api::models::SessionSegmentResponse,
            crate::api::models::PageResponse<crate::api::models::SessionSegmentResponse>,
            SessionDanmuStatisticsResponse,
            DanmuRatePoint,
            DanmuTopTalker,
            DanmuWordFrequency,
            PaginatedResponse<SessionResponse>,
            crate::api::routes::sessions::BatchDeleteRequest,
            crate::api::routes::sessions::BatchDeleteResponse,
            // Template schemas
            CreateTemplateRequest,
            crate::api::routes::templates::CloneTemplateRequest,
            UpdateTemplateRequest,
            TemplateResponse,
            PaginatedResponse<TemplateResponse>,
            // Pipeline schemas
            JobResponse,
            PaginatedResponse<JobResponse>,
            PipelineStatsResponse,
            crate::api::models::StalledOperationResponse,
            crate::api::models::StalledOperationListResponse,
            crate::api::models::UploadRecordResponse,
            crate::api::models::UploadRecordListResponse,
            crate::api::models::MediaOutputUploadInfo,
            crate::api::routes::pipeline::MediaOutputSummaryResponse,
            crate::api::routes::pipeline::MediaOutputTypeSummaryResponse,
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
            crate::api::schemas::EngineConfigurationDbModel,
            crate::api::schemas::EngineType,
            // Notification schemas
            CreateChannelRequest,
            crate::api::routes::notifications::WebPushPublicKeyResponse,
            crate::api::routes::notifications::WebPushSubscriptionResponse,
            crate::api::routes::notifications::WebPushSubscriptionJson,
            crate::api::routes::notifications::WebPushSubscriptionKeys,
            crate::api::routes::notifications::SubscribeWebPushRequest,
            crate::api::routes::notifications::UnsubscribeWebPushRequest,
            UpdateChannelRequest,
            UpdateSubscriptionsRequest,
            crate::api::routes::notifications::ListEventsQuery,
            crate::api::schemas::NotificationChannelDbModel,
            crate::api::schemas::ChannelType,
            crate::api::schemas::NotificationEventLogDbModel,
            crate::api::schemas::NotificationEventTypeInfo,
            crate::api::schemas::NotificationChannelInstance,
            // Job preset schemas
            CreatePresetRequest,
            UpdatePresetRequest,
            ClonePresetRequest,
            PresetListResponse,
            crate::api::schemas::JobPreset,
            // Export/Import schemas
            ConfigExport,
            ImportRequest,
            ImportMode,
            ImportResult,
            ImportStats,
            // Credentials schemas
            crate::api::schemas::CredentialSelection,
            crate::api::schemas::CredentialOwner,
            crate::api::schemas::CredentialProfileSummary,
            crate::api::schemas::CredentialProfileHealth,
            crate::api::schemas::CredentialAttention,
            crate::api::schemas::AttentionReason,
            crate::api::schemas::CredentialBlock,
            crate::api::schemas::BigoNotificationStatus,
            crate::api::schemas::StreamerNotificationMethod,
            crate::api::schemas::ListenerState,
            crate::api::schemas::MonitoringMethod,
            crate::api::schemas::PollingReason,
            crate::api::schemas::CredentialMaterial,
            crate::api::schemas::CredentialBinding,
            crate::api::schemas::ResolvedCredentialPolicy,
            crate::api::schemas::CredentialLoginTarget,
            crate::api::schemas::CredentialLoginGenerated,
            crate::api::schemas::CredentialLoginReceipt,
            crate::api::routes::credential_profiles::CreateProfileRequest,
            crate::api::routes::credential_profiles::UpdateProfileRequest,
            crate::api::routes::credential_profiles::DeleteProfileRequest,
            crate::api::routes::credential_profiles::ProfileDetail,
            crate::api::schemas::ProfileReferences,
            crate::api::schemas::SelectionReference,
            crate::api::schemas::RecordingReference,
            crate::api::routes::credential_profiles::ProfileCapabilities,
            crate::api::routes::credential_profiles::PlatformCredentialCapabilities,
            crate::api::schemas::ProviderCapabilities,
            // Proxy schemas
            crate::api::schemas::ProxyRoute,
            crate::api::schemas::RouteKind,
            crate::api::schemas::RouteSource,
            crate::api::schemas::ProxyName,
            crate::api::schemas::ProxyReferences,
            crate::api::schemas::NamedReference,
            crate::api::schemas::TemplateReference,
            crate::api::schemas::AccountReference,
            crate::api::schemas::SystemProxySummary,
            crate::api::schemas::ProbeOutcome,
            crate::api::schemas::ProbeErrorKind,
            crate::api::routes::proxies::ProxyResponse,
            crate::api::routes::proxies::ProxyDetailResponse,
            crate::api::routes::proxies::CreateProxyRequest,
            crate::api::routes::proxies::UpdateProxyRequest,
            crate::api::routes::proxies::TestProxyRequest,
            crate::api::routes::proxies::EffectiveRouteResponse,
            crate::api::backup::ProxyExport,
            crate::api::backup::BackupRoute,
            crate::api::routes::credential_profiles::EffectiveCredentialSelection,
            crate::api::schemas::StreamerSite,
            crate::api::routes::parse::RenewPlaybackRequest,
            crate::api::playback_context::ManagedPlayback,
            crate::api::playback_context::PlaybackStream,
            // BaiduPCS-Go tool schemas
            BaiduPcsToolRequest,
            BaiduPcsStatusResponse,
            BaiduPcsLoginRequest,
            BaiduPcsLoginResponse,
            BaiduPcsLogoutResponse,
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
            DagListStatsResponse,
            DagRetryResponse,
            DagCancelResponse,
            DagStatsResponse,
            BatchDagAction,
            BatchDagRequest,
            BatchDagItemResult,
            BatchDagResponse,
            BatchDeleteOutputsRequest,
            BatchDeleteOutputsResponse,
            DeleteOutputResponse,
            InterruptedOutputResponse,
            InterruptedOutputListResponse,
            DeleteInterruptedOutputResponse,
            BatchDeleteInterruptedOutputsRequest,
            ValidateDagRequest,
            ValidateDagResponse,
            crate::api::schemas::DagPipelineDefinition,
            crate::api::schemas::DagStep,
            crate::api::schemas::PipelineStep,
        )
    ),
    security(
        ("bearer_auth" = [])
    ),
    modifiers(&SecurityAddon)
)]
pub struct ApiDoc;

/// Security scheme addon for Bearer authentication with a JWT or API key.
struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer_auth",
                utoipa::openapi::security::SecurityScheme::Http(
                    utoipa::openapi::security::HttpBuilder::new()
                        .scheme(utoipa::openapi::security::HttpAuthScheme::Bearer)
                        .bearer_format("JWT or srec_ API key")
                        .description(Some(
                            "JWT access token, or an API key (`srec_...`) created via /api/auth/api-keys",
                        ))
                        .build(),
                ),
            );
        }
    }
}
