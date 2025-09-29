use crate::database::models;
use crate::domain::types::SystemEvent;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct NotificationSubscription {
    pub channel_id: String,
    pub event_name: SystemEvent,
}

impl From<models::NotificationSubscription> for NotificationSubscription {
    fn from(model: models::NotificationSubscription) -> Self {
        Self {
            channel_id: model.channel_id,
            event_name: SystemEvent::from_str(&model.event_name).expect("Invalid event name"),
        }
    }
}

impl From<&NotificationSubscription> for models::NotificationSubscription {
    fn from(domain_subscription: &NotificationSubscription) -> Self {
        Self {
            channel_id: domain_subscription.channel_id.clone(),
            event_name: domain_subscription.event_name.to_string(),
        }
    }
}
