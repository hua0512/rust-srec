use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamerState {
    NotLive,
    Live,
    OutOfSchedule,
    OutOfSpace,
    FatalError,
    Cancelled,
    NotFound,
    InspectingLive,
    TemporalDisabled,
}

impl FromStr for StreamerState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "NotLive" => Ok(StreamerState::NotLive),
            "Live" => Ok(StreamerState::Live),
            "OutOfSchedule" => Ok(StreamerState::OutOfSchedule),
            "OutOfSpace" => Ok(StreamerState::OutOfSpace),
            "FatalError" => Ok(StreamerState::FatalError),
            "Cancelled" => Ok(StreamerState::Cancelled),
            "NotFound" => Ok(StreamerState::NotFound),
            "InspectingLive" => Ok(StreamerState::InspectingLive),
            "TemporalDisabled" => Ok(StreamerState::TemporalDisabled),
            _ => Err(format!("'{}' is not a valid StreamerState", s)),
        }
    }
}

impl ToString for StreamerState {
    fn to_string(&self) -> String {
        match self {
            StreamerState::NotLive => "NotLive".to_string(),
            StreamerState::Live => "Live".to_string(),
            StreamerState::OutOfSchedule => "OutOfSchedule".to_string(),
            StreamerState::OutOfSpace => "OutOfSpace".to_string(),
            StreamerState::FatalError => "FatalError".to_string(),
            StreamerState::Cancelled => "Cancelled".to_string(),
            StreamerState::NotFound => "NotFound".to_string(),
            StreamerState::InspectingLive => "InspectingLive".to_string(),
            StreamerState::TemporalDisabled => "TemporalDisabled".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterType {
    TimeBased,
    Keyword,
    Category,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaType {
    Video,
    Audio,
    Thumbnail,
    DanmuXml,
}

impl std::str::FromStr for MediaType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "Video" => Ok(MediaType::Video),
            "Audio" => Ok(MediaType::Audio),
            "Thumbnail" => Ok(MediaType::Thumbnail),
            "DanmuXml" => Ok(MediaType::DanmuXml),
            _ => Err(format!("Unknown media type: {}", s)),
        }
    }
}

impl ToString for MediaType {
    fn to_string(&self) -> String {
        match self {
            MediaType::Video => "Video".to_string(),
            MediaType::Audio => "Audio".to_string(),
            MediaType::Thumbnail => "Thumbnail".to_string(),
            MediaType::DanmuXml => "DanmuXml".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

impl FromStr for JobStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Pending" => Ok(JobStatus::Pending),
            "InProgress" => Ok(JobStatus::InProgress),
            "Completed" => Ok(JobStatus::Completed),
            "Failed" => Ok(JobStatus::Failed),
            _ => Err(format!("'{}' is not a valid JobStatus", s)),
        }
    }
}

impl ToString for JobStatus {
    fn to_string(&self) -> String {
        match self {
            JobStatus::Pending => "Pending".to_string(),
            JobStatus::InProgress => "InProgress".to_string(),
            JobStatus::Completed => "Completed".to_string(),
            JobStatus::Failed => "Failed".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobType {
    Remux,
    Upload,
    Transcode,
    Cleanup,
}

impl FromStr for JobType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Remux" => Ok(JobType::Remux),
            "Upload" => Ok(JobType::Upload),
            "Transcode" => Ok(JobType::Transcode),
            "Cleanup" => Ok(JobType::Cleanup),
            _ => Err(format!("'{}' is not a valid JobType", s)),
        }
    }
}

impl ToString for JobType {
    fn to_string(&self) -> String {
        match self {
            JobType::Remux => "Remux".to_string(),
            JobType::Upload => "Upload".to_string(),
            JobType::Transcode => "Transcode".to_string(),
            JobType::Cleanup => "Cleanup".to_string(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeyRole {
    Admin,
    Readonly,
}

impl FromStr for ApiKeyRole {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Admin" => Ok(ApiKeyRole::Admin),
            "Readonly" => Ok(ApiKeyRole::Readonly),
            _ => Err(format!("'{}' is not a valid ApiKeyRole", s)),
        }
    }
}

impl ToString for ApiKeyRole {
    fn to_string(&self) -> String {
        match self {
            ApiKeyRole::Admin => "Admin".to_string(),
            ApiKeyRole::Readonly => "Readonly".to_string(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationChannelType {
    Discord,
    Email,
}

impl FromStr for NotificationChannelType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Discord" => Ok(NotificationChannelType::Discord),
            "Email" => Ok(NotificationChannelType::Email),
            _ => Err(format!("'{}' is not a valid NotificationChannelType", s)),
        }
    }
}

impl ToString for NotificationChannelType {
    fn to_string(&self) -> String {
        match self {
            NotificationChannelType::Discord => "Discord".to_string(),
            NotificationChannelType::Email => "Email".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum NotificationChannelSettings {
    Discord { webhook_url: String },
    Email { recipient_email: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SystemEvent {
    StreamOnline,
    StreamOffline,
    DownloadStarted,
    DownloadCompleted,
    DownloadError,
    FatalError,
    OutOfSpace,
    PipelineStarted,
    PipelineCompleted,
    PipelineFailed,
}

impl FromStr for SystemEvent {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "StreamOnline" => Ok(SystemEvent::StreamOnline),
            "StreamOffline" => Ok(SystemEvent::StreamOffline),
            "DownloadStarted" => Ok(SystemEvent::DownloadStarted),
            "DownloadCompleted" => Ok(SystemEvent::DownloadCompleted),
            "DownloadError" => Ok(SystemEvent::DownloadError),
            "FatalError" => Ok(SystemEvent::FatalError),
            "OutOfSpace" => Ok(SystemEvent::OutOfSpace),
            "PipelineStarted" => Ok(SystemEvent::PipelineStarted),
            "PipelineCompleted" => Ok(SystemEvent::PipelineCompleted),
            "PipelineFailed" => Ok(SystemEvent::PipelineFailed),
            _ => Err(format!("'{}' is not a valid SystemEvent", s)),
        }
    }
}

impl ToString for SystemEvent {
    fn to_string(&self) -> String {
        match self {
            SystemEvent::StreamOnline => "StreamOnline".to_string(),
            SystemEvent::StreamOffline => "StreamOffline".to_string(),
            SystemEvent::DownloadStarted => "DownloadStarted".to_string(),
            SystemEvent::DownloadCompleted => "DownloadCompleted".to_string(),
            SystemEvent::DownloadError => "DownloadError".to_string(),
            SystemEvent::FatalError => "FatalError".to_string(),
            SystemEvent::OutOfSpace => "OutOfSpace".to_string(),
            SystemEvent::PipelineStarted => "PipelineStarted".to_string(),
            SystemEvent::PipelineCompleted => "PipelineCompleted".to_string(),
            SystemEvent::PipelineFailed => "PipelineFailed".to_string(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UploadStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

impl FromStr for UploadStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Pending" => Ok(UploadStatus::Pending),
            "InProgress" => Ok(UploadStatus::InProgress),
            "Completed" => Ok(UploadStatus::Completed),
            "Failed" => Ok(UploadStatus::Failed),
            _ => Err(format!("'{}' is not a valid UploadStatus", s)),
        }
    }
}

impl ToString for UploadStatus {
    fn to_string(&self) -> String {
        match self {
            UploadStatus::Pending => "Pending".to_string(),
            UploadStatus::InProgress => "InProgress".to_string(),
            UploadStatus::Completed => "Completed".to_string(),
            UploadStatus::Failed => "Failed".to_string(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamerUrl(pub String);

impl From<String> for StreamerUrl {
    fn from(s: String) -> Self {
        Self(s)
    }
}

#[derive(Debug, Clone)]
pub struct Filter {
    pub filter_type: FilterType,
    pub config: Value,
}

impl From<crate::database::models::Filter> for Filter {
    fn from(model: crate::database::models::Filter) -> Self {
        let filter_type = match model.filter_type.as_str() {
            "TimeBased" => FilterType::TimeBased,
            "Keyword" => FilterType::Keyword,
            "Category" => FilterType::Category,
            _ => panic!("Unknown filter type"),
        };
        Self {
            filter_type,
            config: serde_json::from_str(&model.config).unwrap(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ProxyConfig {
    pub enabled: bool,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DownloadRetryPolicy {
    pub max_retries: u32,
    pub delay_ms: u64,
    pub backoff_factor: f32,
}

impl Default for DownloadRetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            delay_ms: 10000,
            backoff_factor: 2.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "strategy", rename_all = "camelCase")]
pub enum DanmuSamplingConfig {
    Fixed {
        interval_secs: u64,
    },
    Dynamic {
        min_interval_secs: u64,
        max_interval_secs: u64,
        target_danmus_per_sample: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DownloadEngineType {
    Ffmpeg,
    Streamlink,
    Mesio,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlatformOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetch_delay_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_delay_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookies: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_specific_config: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_config: Option<ProxyConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_retry_policy: Option<DownloadRetryPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_engine: Option<String>,
}

pub type PlatformOverrides = HashMap<String, PlatformOverride>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EngineOverride {
    pub engine_type: String,
    pub config: serde_json::Value,
}

pub type EnginesOverride = HashMap<String, EngineOverride>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventHook {
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

pub type EventHooks = HashMap<String, EventHook>;
