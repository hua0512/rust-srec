use async_trait::async_trait;
use crate::notification::events::SystemEvent;
use super::Notifier;

pub struct EmailNotifier;

impl EmailNotifier {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EmailNotifier {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Notifier for EmailNotifier {
    async fn notify(&self, event: &SystemEvent) {
        println!("Sending Email notification for event: {:?}", event);
    }
}