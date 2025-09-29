use crate::database::{converters, models};
use crate::domain::types::ApiKeyRole;
use chrono::{DateTime, Utc};
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct ApiKey {
    pub id: String,
    pub key_hash: String,
    pub name: String,
    pub role: ApiKeyRole,
    pub created_at: DateTime<Utc>,
}

impl From<models::ApiKey> for ApiKey {
    fn from(model: models::ApiKey) -> Self {
        Self {
            id: model.id,
            key_hash: model.key_hash,
            name: model.name,
            role: ApiKeyRole::from_str(&model.role).expect("Invalid role"),
            created_at: converters::string_to_datetime(&model.created_at)
                .expect("Invalid created_at format"),
        }
    }
}

impl From<&ApiKey> for models::ApiKey {
    fn from(domain_api_key: &ApiKey) -> Self {
        Self {
            id: domain_api_key.id.clone(),
            key_hash: domain_api_key.key_hash.clone(),
            name: domain_api_key.name.clone(),
            role: domain_api_key.role.to_string(),
            created_at: converters::datetime_to_string(&domain_api_key.created_at),
        }
    }
}
