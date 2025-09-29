use super::types::MediaType;
use chrono::{DateTime, Utc};

pub struct MediaOutput {
    pub id: String,
    pub session_id: String,
    pub file_path: String,
    pub file_type: MediaType,
    pub size_bytes: u64,
    pub parent_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<crate::database::models::MediaOutput> for MediaOutput {
    fn from(model: crate::database::models::MediaOutput) -> Self {
        Self {
            id: model.id,
            session_id: model.session_id,
            file_path: model.file_path,
            file_type: model.file_type.parse().unwrap(),
            size_bytes: model.size_bytes as u64,
            parent_id: model.parent_media_output_id,
            created_at: model.created_at.parse().unwrap(),
        }
    }
}
