use crate::database::{converters, models};
use crate::domain::types::UploadStatus;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct UploadRecord {
    pub id: String,
    pub media_output_id: String,
    pub platform: String,
    pub remote_path: String,
    pub status: UploadStatus,
    pub metadata: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl From<models::UploadRecord> for UploadRecord {
    fn from(model: models::UploadRecord) -> Self {
        Self {
            id: model.id,
            media_output_id: model.media_output_id,
            platform: model.platform,
            remote_path: model.remote_path,
            status: UploadStatus::from_str(&model.status).expect("Invalid upload status"),
            metadata: converters::optional_string_to_json(&model.metadata)
                .expect("Invalid metadata JSON"),
            created_at: converters::string_to_datetime(&model.created_at)
                .expect("Invalid created_at format"),
            completed_at: converters::optional_string_to_datetime(&model.completed_at)
                .expect("Invalid completed_at format"),
        }
    }
}

impl From<&UploadRecord> for models::UploadRecord {
    fn from(domain_record: &UploadRecord) -> Self {
        Self {
            id: domain_record.id.clone(),
            media_output_id: domain_record.media_output_id.clone(),
            platform: domain_record.platform.clone(),
            remote_path: domain_record.remote_path.clone(),
            status: domain_record.status.to_string(),
            metadata: converters::optional_json_to_string(&domain_record.metadata)
                .expect("Failed to serialize metadata"),
            created_at: converters::datetime_to_string(&domain_record.created_at),
            completed_at: converters::optional_datetime_to_string(&domain_record.completed_at),
        }
    }
}
