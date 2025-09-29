use crate::database::models;
use crate::domain::types::{NotificationChannelSettings, NotificationChannelType};
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct NotificationChannel {
    pub id: String,
    pub name: String,
    pub channel_type: NotificationChannelType,
    pub settings: NotificationChannelSettings,
}

impl From<models::NotificationChannel> for NotificationChannel {
    fn from(model: models::NotificationChannel) -> Self {
        Self {
            id: model.id,
            name: model.name,
            channel_type: NotificationChannelType::from_str(&model.channel_type)
                .expect("Invalid channel type"),
            settings: serde_json::from_str(&model.settings)
                .expect("Failed to deserialize settings"),
        }
    }
}

impl From<&NotificationChannel> for models::NotificationChannel {
    fn from(domain_channel: &NotificationChannel) -> Self {
        Self {
            id: domain_channel.id.clone(),
            name: domain_channel.name.clone(),
            channel_type: domain_channel.channel_type.to_string(),
            settings: serde_json::to_string(&domain_channel.settings)
                .expect("Failed to serialize settings"),
        }
    }
}
