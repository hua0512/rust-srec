use async_trait::async_trait;
use crate::notification::events::SystemEvent;
use super::Notifier;

pub struct DiscordNotifier;

impl DiscordNotifier {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DiscordNotifier {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Notifier for DiscordNotifier {
    async fn notify(&self, event: &SystemEvent) {
        println!("Sending Discord notification for event: {:?}", event);
    }
}