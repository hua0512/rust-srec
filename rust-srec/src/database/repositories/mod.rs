pub mod api_key_repository;
pub mod engine_config_repository;
pub mod errors;
pub mod filter_repository;
pub mod global_config_repository;
pub mod job_repository;
pub mod live_session_repository;
pub mod media_output_repository;
pub mod notification_channel_repository;
pub mod notification_subscription_repository;
pub mod platform_config_repository;
pub mod streamer_repository;
pub mod template_config_repository;
pub mod upload_record_repository;

pub use self::{
    api_key_repository::{ApiKeyRepository, SqliteApiKeyRepository},
    engine_config_repository::{EngineConfigRepository, SqliteEngineConfigRepository},
    errors::{RepositoryError, RepositoryResult},
    filter_repository::{FilterRepository, SqliteFilterRepository},
    global_config_repository::{GlobalConfigRepository, SqliteGlobalConfigRepository},
    job_repository::{JobRepository, SqliteJobRepository},
    live_session_repository::{LiveSessionRepository, SqliteLiveSessionRepository},
    media_output_repository::{MediaOutputRepository, SqliteMediaOutputRepository},
    notification_channel_repository::{
        NotificationChannelRepository, SqliteNotificationChannelRepository,
    },
    notification_subscription_repository::{
        NotificationSubscriptionRepository, SqliteNotificationSubscriptionRepository,
    },
    platform_config_repository::{PlatformConfigRepository, SqlitePlatformConfigRepository},
    streamer_repository::{SqliteStreamerRepository, StreamerRepository},
    template_config_repository::{SqliteTemplateConfigRepository, TemplateConfigRepository},
    upload_record_repository::{SqliteUploadRecordRepository, UploadRecordRepository},
};
