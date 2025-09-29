use crate::database::{converters, models};
use crate::domain::types::{JobStatus, JobType};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub job_type: JobType,
    pub status: JobStatus,
    pub context: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<models::Job> for Job {
    fn from(model: models::Job) -> Self {
        Self {
            id: model.id,
            job_type: JobType::from_str(&model.job_type).expect("Invalid job type"),
            status: JobStatus::from_str(&model.status).expect("Invalid job status"),
            context: serde_json::from_str(&model.context).expect("Invalid context JSON"),
            created_at: converters::string_to_datetime(&model.created_at)
                .expect("Invalid created_at format"),
            updated_at: converters::string_to_datetime(&model.updated_at)
                .expect("Invalid updated_at format"),
        }
    }
}

impl From<&Job> for models::Job {
    fn from(domain_job: &Job) -> Self {
        Self {
            id: domain_job.id.clone(),
            job_type: domain_job.job_type.to_string(),
            status: domain_job.status.to_string(),
            context: serde_json::to_string(&domain_job.context).expect("Failed to serialize context"),
            created_at: converters::datetime_to_string(&domain_job.created_at),
            updated_at: converters::datetime_to_string(&domain_job.updated_at),
        }
    }
}
