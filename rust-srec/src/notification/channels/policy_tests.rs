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
async fn all_channels_apply_the_same_enabled_and_priority_contract() {
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
            // These configurations cannot perform network IO: filtering must
            // return before malformed endpoints/transports are constructed.
            // `send_rendered` and `test` leave `min_priority` to their callers.
            if !accepted {
                channel.send(&event).await.unwrap();
            }
            if !enabled {
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
