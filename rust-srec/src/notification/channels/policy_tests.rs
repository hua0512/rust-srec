use super::*;

fn channels(
    enabled: bool,
    min_priority: NotificationPriority,
) -> Vec<Box<dyn NotificationChannel>> {
    vec![
        Box::new(DiscordChannel::new(DiscordConfig {
            enabled,
            min_priority,
            webhook_url: "invalid://discord".into(),
            ..Default::default()
        })),
        Box::new(EmailChannel::new(EmailConfig {
            enabled,
            min_priority,
            smtp_host: "invalid".into(),
            from_address: "sender@example.test".into(),
            to_addresses: vec!["receiver@example.test".into()],
            ..Default::default()
        })),
        Box::new(GotifyChannel::new(GotifyConfig {
            enabled,
            min_priority,
            server_url: "invalid://gotify".into(),
            app_token: "test".into(),
            ..Default::default()
        })),
        Box::new(TelegramChannel::new(TelegramConfig {
            enabled,
            min_priority,
            bot_token: "test".into(),
            chat_id: "test".into(),
            ..Default::default()
        })),
        Box::new(WebhookChannel::new(WebhookConfig {
            enabled,
            min_priority,
            url: "invalid://webhook".into(),
            ..Default::default()
        })),
    ]
}

fn event() -> NotificationEvent {
    NotificationEvent::SystemStartup {
        version: "contract".into(),
        timestamp: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn all_channels_apply_the_same_enabled_and_priority_contract_to_both_send_paths() {
    let event = event();
    let rendered = RenderedEvent::new(&event, "en");
    for (enabled, threshold, accepted) in [
        (false, NotificationPriority::Low, false),
        (true, NotificationPriority::High, false),
        (true, NotificationPriority::Normal, true),
        (true, NotificationPriority::Low, true),
    ] {
        for channel in channels(enabled, threshold) {
            assert_eq!(
                channel.accepts(&event),
                accepted,
                "{}",
                channel.channel_type()
            );
            if !accepted {
                // These configurations cannot perform network IO: filtering must
                // return before malformed endpoints/transports are constructed.
                channel.send(&event).await.unwrap();
                channel.send_rendered(&event, &rendered).await.unwrap();
                channel.test().await.unwrap();
            }
        }
    }
    assert_eq!(
        EmailConfig::default().min_priority,
        NotificationPriority::High
    );
    assert_eq!(
        DiscordConfig::default().min_priority,
        NotificationPriority::Normal
    );
    assert_eq!(
        GotifyConfig::default().min_priority,
        NotificationPriority::Normal
    );
    assert_eq!(
        TelegramConfig::default().min_priority,
        NotificationPriority::Normal
    );
    assert_eq!(
        WebhookConfig::default().min_priority,
        NotificationPriority::Normal
    );
}

#[derive(Default)]
struct ExternalChannel {
    events: parking_lot::Mutex<Vec<NotificationEvent>>,
}

#[async_trait]
impl NotificationChannel for ExternalChannel {
    fn channel_type(&self) -> &'static str {
        "external"
    }
    fn is_enabled(&self) -> bool {
        true
    }
    async fn send(&self, event: &NotificationEvent) -> Result<()> {
        self.events.lock().push(event.clone());
        Ok(())
    }
}

#[tokio::test]
async fn external_send_implementations_keep_default_test_and_rendered_delivery() {
    let channel = ExternalChannel::default();
    let before = chrono::Utc::now();
    channel.test().await.unwrap();
    let event = event();
    channel
        .send_rendered(&event, &RenderedEvent::new(&event, "zh-CN"))
        .await
        .unwrap();
    let events = channel.events.lock();
    assert_eq!(events.len(), 2);
    assert!(
        matches!(&events[0], NotificationEvent::SystemStartup { version, timestamp } if version == "test" && *timestamp >= before && *timestamp <= chrono::Utc::now())
    );
    assert_eq!(
        serde_json::to_value(&events[1]).unwrap(),
        serde_json::to_value(event).unwrap()
    );
}
