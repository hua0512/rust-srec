use super::*;
use crate::notification::NotificationServiceConfig;

#[test]
fn nested_notification_config_debug_redacts_credentials_but_serialization_preserves_them() {
    let channels = vec![
        ChannelConfig::Discord(DiscordConfig {
            name: Some("visible-discord".to_string()),
            webhook_url: "https://discord.example/api/webhooks/discord-credential".to_string(),
            avatar_url: Some("https://cdn.example/avatar?token=avatar-credential".to_string()),
            ..Default::default()
        }),
        ChannelConfig::Email(EmailConfig {
            smtp_host: "mail.example.com".to_string(),
            smtp_username: Some("smtp-identity".to_string()),
            smtp_password: Some("smtp-credential".to_string()),
            from_address: "private-sender@example.com".to_string(),
            to_addresses: vec!["private-recipient@example.com".to_string()],
            ..Default::default()
        }),
        ChannelConfig::Telegram(TelegramConfig {
            bot_token: "telegram-credential".to_string(),
            chat_id: "private-chat-id".to_string(),
            ..Default::default()
        }),
        ChannelConfig::Gotify(GotifyConfig {
            server_url: "https://gotify-user:gotify-url-password@gotify.example/private-path"
                .to_string(),
            app_token: "gotify-credential".to_string(),
            ..Default::default()
        }),
        ChannelConfig::Webhook(WebhookConfig {
            url:
                "https://hook-user:hook-password@hooks.example/path-credential?key=query-credential"
                    .to_string(),
            headers: vec![(
                "X-Custom-Key".to_string(),
                "custom-header-credential".to_string(),
            )],
            auth: Some(WebhookAuth::Bearer {
                token: "bearer-credential".to_string(),
            }),
            ..Default::default()
        }),
        ChannelConfig::Webhook(WebhookConfig {
            auth: Some(WebhookAuth::Basic {
                username: "basic-identity".to_string(),
                password: "basic-credential".to_string(),
            }),
            ..Default::default()
        }),
        ChannelConfig::Webhook(WebhookConfig {
            auth: Some(WebhookAuth::Header {
                name: "X-Authentication".to_string(),
                value: "auth-header-credential".to_string(),
            }),
            ..Default::default()
        }),
    ];
    let config = NotificationServiceConfig {
        channels,
        ..Default::default()
    };
    let secrets = [
        "discord-credential",
        "avatar-credential",
        "smtp-identity",
        "smtp-credential",
        "private-sender",
        "private-recipient",
        "telegram-credential",
        "private-chat-id",
        "gotify-user",
        "gotify-url-password",
        "private-path",
        "gotify-credential",
        "hook-user",
        "hook-password",
        "path-credential",
        "query-credential",
        "custom-header-credential",
        "bearer-credential",
        "basic-identity",
        "basic-credential",
        "auth-header-credential",
    ];
    for debug in [format!("{config:?}"), format!("{config:#?}")] {
        for secret in secrets {
            assert!(!debug.contains(secret), "Debug leaked {secret}");
        }
        for diagnostic in [
            "visible-discord",
            "mail.example.com",
            "X-Custom-Key",
            "X-Authentication",
            "[REDACTED]",
        ] {
            assert!(
                debug.contains(diagnostic),
                "missing diagnostic {diagnostic}"
            );
        }
    }
    let serialized = serde_json::to_string(&config).unwrap();
    for secret in secrets {
        assert!(
            serialized.contains(secret),
            "redaction must not alter configured credentials"
        );
    }
    let roundtrip: NotificationServiceConfig = serde_json::from_str(&serialized).unwrap();
    assert_eq!(
        serde_json::to_value(roundtrip).unwrap(),
        serde_json::to_value(config).unwrap()
    );
}

#[test]
fn standalone_webhook_auth_debug_redacts_each_auth_variant() {
    for auth in [
        WebhookAuth::Bearer {
            token: "standalone-secret".to_string(),
        },
        WebhookAuth::Basic {
            username: "standalone-user".to_string(),
            password: "standalone-secret".to_string(),
        },
        WebhookAuth::Header {
            name: "X-Key".to_string(),
            value: "standalone-secret".to_string(),
        },
    ] {
        let debug = format!("{auth:?}");
        assert!(!debug.contains("standalone-secret"));
        assert!(!debug.contains("standalone-user"));
        assert!(debug.contains("[REDACTED]"));
    }
}
