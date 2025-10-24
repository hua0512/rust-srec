use async_trait::async_trait;
use crate::notification::events::SystemEvent;

pub mod discord;
pub mod email;

#[async_trait]
pub trait Notifier: Send + Sync {
    async fn notify(&self, event: &SystemEvent);
}