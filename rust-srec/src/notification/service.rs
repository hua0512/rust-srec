use crate::domain::{
    notification_channel::NotificationChannel,
    notification_subscription::NotificationSubscription,
    types::{NotificationChannelType, SystemEvent as DomainSystemEvent},
};
use crate::notification::events::SystemEvent;
use crate::notification::notifiers::{
    discord::DiscordNotifier, email::EmailNotifier, Notifier,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};

pub struct NotificationService {
    subscriptions: Arc<Mutex<HashMap<DomainSystemEvent, Vec<Arc<dyn Notifier>>>>>,
    event_sender: broadcast::Sender<SystemEvent>,
}

impl Default for NotificationService {
    fn default() -> Self {
        let (event_sender, _) = broadcast::channel(100);
        Self {
            subscriptions: Arc::new(Mutex::new(HashMap::new())),
            event_sender,
        }
    }
}

impl NotificationService {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn subscribe(
        &self,
        subscriptions: Vec<NotificationSubscription>,
        channels: Vec<NotificationChannel>,
    ) {
        let mut subs = self.subscriptions.lock().await;
        subs.clear();

        let channel_map: HashMap<String, &NotificationChannel> =
            channels.iter().map(|c| (c.id.clone(), c)).collect();

        for sub in subscriptions {
            if let Some(channel) = channel_map.get(&sub.channel_id) {
                let notifier: Arc<dyn Notifier> = match channel.channel_type {
                    NotificationChannelType::Discord => Arc::new(DiscordNotifier::new()),
                    NotificationChannelType::Email => Arc::new(EmailNotifier::new()),
                };
                subs.entry(sub.event_name.clone()).or_default().push(notifier);
            }
        }
    }

    pub fn get_event_sender(&self) -> broadcast::Sender<SystemEvent> {
        self.event_sender.clone()
    }

    pub async fn run(&self) {
        let mut receiver = self.event_sender.subscribe();
        let subscriptions = self.subscriptions.clone();

        tokio::spawn(async move {
            while let Ok(event) = receiver.recv().await {
                let event_type = match &event {
                    SystemEvent::DownloadCompleted(_, _) => DomainSystemEvent::DownloadCompleted,
                    SystemEvent::FatalError(_) => DomainSystemEvent::FatalError,
                };

                let subs = subscriptions.lock().await;
                if let Some(notifiers) = subs.get(&event_type) {
                    for notifier in notifiers {
                        let notifier = notifier.clone();
                        let event = event.clone();
                        tokio::spawn(async move {
                            notifier.notify(&event).await;
                        });
                    }
                }
            }
        });
    }
}