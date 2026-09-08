//! Email notification channel using SMTP.

use async_trait::async_trait;
use lettre::message::{Mailbox, MultiPart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;
use tracing::debug;

use super::NotificationChannel;
use crate::Result;
use crate::notification::events::{NotificationEvent, NotificationPriority};

#[cfg(test)]
mod smtp_tests;

/// Email channel configuration.
#[derive(Clone, Serialize, Deserialize)]
pub struct EmailConfig {
    /// Stable channel instance identifier (recommended).
    ///
    /// When provided, this is used to derive the runtime channel key so reordering the config
    /// does not reset circuit breaker history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Optional display name for this channel instance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether the channel is enabled.
    pub enabled: bool,
    /// SMTP server host.
    pub smtp_host: String,
    /// SMTP server port.
    pub smtp_port: u16,
    /// SMTP username.
    pub smtp_username: Option<String>,
    /// SMTP password.
    pub smtp_password: Option<String>,
    /// Use TLS.
    pub use_tls: bool,
    /// Sender email address.
    pub from_address: String,
    /// Recipient email addresses.
    pub to_addresses: Vec<String>,
    /// Minimum priority level to send (default: High).
    #[serde(default = "default_email_priority")]
    pub min_priority: NotificationPriority,
    /// Language for the rendered title and body; `None` follows the process-wide locale.
    /// See `notification::service::parse_channel_locale`.
    #[serde(default)]
    pub locale: Option<String>,
}

impl std::fmt::Debug for EmailConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmailConfig")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field("smtp_host", &self.smtp_host)
            .field("smtp_port", &self.smtp_port)
            .field(
                "smtp_username",
                &self.smtp_username.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "smtp_password",
                &self.smtp_password.as_ref().map(|_| "[REDACTED]"),
            )
            .field("use_tls", &self.use_tls)
            .field("from_address", &"[REDACTED]")
            .field("recipient_count", &self.to_addresses.len())
            .field("min_priority", &self.min_priority)
            .field("locale", &self.locale)
            .finish()
    }
}

fn default_email_priority() -> NotificationPriority {
    NotificationPriority::High
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            id: None,
            name: None,
            enabled: false,
            smtp_host: "localhost".to_string(),
            smtp_port: 587,
            smtp_username: None,
            smtp_password: None,
            use_tls: true,
            from_address: String::new(),
            to_addresses: Vec::new(),
            min_priority: NotificationPriority::High,
            locale: None,
        }
    }
}

/// Email notification channel.
pub struct EmailChannel {
    config: EmailConfig,
    // Each immutable channel owns its pool; admitted deliveries retain that
    // channel even when a configuration reload publishes a replacement.
    transport: OnceCell<AsyncSmtpTransport<Tokio1Executor>>,
}

struct EmailContent {
    title: String,
    description: String,
    priority: NotificationPriority,
    event_type: &'static str,
    timestamp: String,
}

impl EmailChannel {
    /// Create a new Email channel.
    pub fn new(config: EmailConfig) -> Self {
        Self {
            config,
            transport: OnceCell::new(),
        }
    }

    fn render(&self, event: &NotificationEvent) -> EmailContent {
        // Resolve the process locale once so every MIME part uses one language.
        let locale = self
            .config
            .locale
            .clone()
            .unwrap_or_else(crate::i18n::current_locale);
        EmailContent {
            title: event.title_in(&locale),
            description: event.description_in(&locale),
            priority: event.priority(),
            event_type: event.event_type(),
            timestamp: event.timestamp().to_rfc3339(),
        }
    }

    /// Build the email subject.
    fn build_subject(&self, content: &EmailContent) -> String {
        format!("[rust-srec] {}", content.title)
    }

    /// Build the email body (plain text).
    fn build_body_text(&self, content: &EmailContent) -> String {
        format!(
            "{}\n\n{}\n\nPriority: {}\nType: {}\nTime: {}",
            content.title,
            content.description,
            content.priority,
            content.event_type,
            content.timestamp
        )
    }

    /// Build the email body (HTML).
    fn build_body_html(&self, content: &EmailContent) -> String {
        let priority_color = match content.priority {
            NotificationPriority::Low => "#808080",
            NotificationPriority::Normal => "#3498db",
            NotificationPriority::High => "#f39c12",
            NotificationPriority::Critical => "#e74c3c",
        };

        let title = escape_html(&content.title);
        let description = escape_html(&content.description);
        let priority = escape_html(&content.priority.to_string());
        let event_type = escape_html(content.event_type);
        let timestamp = escape_html(&content.timestamp);

        format!(
            r#"<!DOCTYPE html>
<html>
<head>
    <style>
        body {{ font-family: Arial, sans-serif; margin: 20px; }}
        .header {{ background-color: {}; color: white; padding: 15px; border-radius: 5px; }}
        .content {{ padding: 20px; background-color: #f9f9f9; border-radius: 5px; margin-top: 10px; }}
        .footer {{ color: #666; font-size: 12px; margin-top: 20px; }}
    </style>
</head>
<body>
    <div class="header">
        <h2>{}</h2>
    </div>
    <div class="content">
        <p>{}</p>
    </div>
    <div class="footer">
        <p>Priority: {} | Type: {} | Time: {}</p>
    </div>
</body>
</html>"#,
            priority_color, title, description, priority, event_type, timestamp
        )
    }

    fn build_message(&self, event: &NotificationEvent) -> Result<Message> {
        let from = parse_mailbox(&self.config.from_address, "sender")?;
        let content = self.render(event);
        let mut builder = Message::builder()
            .from(from)
            .subject(self.build_subject(&content));

        for address in &self.config.to_addresses {
            builder = builder.to(parse_mailbox(address, "recipient")?);
        }

        builder
            .multipart(MultiPart::alternative_plain_html(
                self.build_body_text(&content),
                self.build_body_html(&content),
            ))
            .map_err(|error| crate::Error::config(format!("Invalid email message: {error}")))
    }

    fn build_transport(&self) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
        let host = self.config.smtp_host.trim();
        if host.is_empty() {
            return Err(crate::Error::config("SMTP host cannot be empty"));
        }

        let mut builder = if self.config.use_tls {
            if self.config.smtp_port == 465 {
                AsyncSmtpTransport::<Tokio1Executor>::relay(host)
            } else {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)
            }
            .map_err(|error| {
                crate::Error::config(format!("Invalid SMTP TLS configuration: {error}"))
            })?
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
        };

        builder = builder.port(self.config.smtp_port);
        match (&self.config.smtp_username, &self.config.smtp_password) {
            (Some(username), Some(password))
                if !username.trim().is_empty() && !password.is_empty() =>
            {
                builder = builder.credentials(Credentials::new(
                    username.trim().to_string(),
                    password.clone(),
                ));
            }
            (None, None) => {}
            (Some(username), None) if username.trim().is_empty() => {}
            (None, Some(password)) if password.is_empty() => {}
            _ => {
                return Err(crate::Error::config(
                    "SMTP username and password must either both be configured or both be absent",
                ));
            }
        }

        Ok(builder.build())
    }
}

#[async_trait]
impl NotificationChannel for EmailChannel {
    fn channel_type(&self) -> &'static str {
        "email"
    }

    fn is_enabled(&self) -> bool {
        self.config.enabled
            && !self.config.smtp_host.is_empty()
            && !self.config.from_address.is_empty()
            && !self.config.to_addresses.is_empty()
    }

    async fn send(&self, event: &NotificationEvent) -> Result<()> {
        if !self.is_enabled() {
            return Ok(());
        }

        // Check priority filter
        if event.priority() < self.config.min_priority {
            debug!(
                "Skipping email notification for {} (priority {} < {})",
                event.event_type(),
                event.priority(),
                self.config.min_priority
            );
            return Ok(());
        }

        let message = self.build_message(event)?;
        let transport = self
            .transport
            .get_or_try_init(|| async { self.build_transport() })
            .await?;
        transport.send(message).await.map_err(|error| {
            crate::Error::Other(format!("Email delivery via SMTP failed: {error}"))
        })?;

        debug!(
            event_type = event.event_type(),
            "Email notification delivered"
        );
        Ok(())
    }

    async fn test(&self) -> Result<()> {
        let test_event = NotificationEvent::SystemStartup {
            version: "test".to_string(),
            timestamp: chrono::Utc::now(),
        };
        self.send(&test_event).await
    }
}

fn parse_mailbox(address: &str, kind: &str) -> Result<Mailbox> {
    address.trim().parse().map_err(|error| {
        crate::Error::config(format!("Invalid email {kind} address '{address}': {error}"))
    })
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_email_channel_disabled() {
        let config = EmailConfig::default();
        // Email is the one channel whose default `min_priority` is `High`
        // rather than `Normal`; `EmailChannel::send` drops any event below
        // it, so a default mailbox only ever receives High and Critical.
        assert_eq!(config.min_priority, NotificationPriority::High);

        let channel = EmailChannel::new(config);
        assert!(!channel.is_enabled());
    }

    #[test]
    fn test_build_subject() {
        let config = EmailConfig::default();
        let channel = EmailChannel::new(config);

        let event = NotificationEvent::StreamOnline {
            streamer_id: "123".to_string(),
            streamer_name: "TestStreamer".to_string(),
            title: "Playing Games".to_string(),
            category: None,
            timestamp: chrono::Utc::now(),
        };

        let subject = channel.build_subject(&channel.render(&event));
        assert!(subject.contains("rust-srec"));
        assert!(subject.contains("TestStreamer"));
    }

    #[test]
    fn message_contains_all_recipients_and_multipart_bodies() {
        let config = EmailConfig {
            enabled: true,
            smtp_host: "smtp.example.com".to_string(),
            from_address: "rust-srec@example.com".to_string(),
            to_addresses: vec![
                "first@example.com".to_string(),
                "second@example.com".to_string(),
            ],
            ..EmailConfig::default()
        };
        let channel = EmailChannel::new(config);
        let event = NotificationEvent::StreamOnline {
            streamer_id: "123".to_string(),
            streamer_name: "TestStreamer".to_string(),
            title: "Playing Games".to_string(),
            category: None,
            timestamp: chrono::Utc::now(),
        };

        let formatted = String::from_utf8(channel.build_message(&event).unwrap().formatted())
            .expect("message should be UTF-8");
        assert!(formatted.contains("first@example.com"));
        assert!(formatted.contains("second@example.com"));
        assert!(formatted.contains("multipart/alternative"));
        assert!(formatted.contains("TestStreamer"));
    }

    #[test]
    fn html_body_escapes_event_content() {
        let channel = EmailChannel::new(EmailConfig::default());
        let event = NotificationEvent::StreamOnline {
            streamer_id: "123".to_string(),
            streamer_name: "<script>alert('x')</script>".to_string(),
            title: "A & B".to_string(),
            category: None,
            timestamp: chrono::Utc::now(),
        };

        let html = channel.build_body_html(&channel.render(&event));
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("A &amp; B"));
    }

    #[test]
    fn smtp_credentials_must_be_configured_as_a_pair() {
        let config = EmailConfig {
            smtp_host: "smtp.example.com".to_string(),
            smtp_username: Some("user".to_string()),
            smtp_password: None,
            ..EmailConfig::default()
        };

        let error = EmailChannel::new(config)
            .build_transport()
            .expect_err("partial credentials must be rejected");
        assert!(error.to_string().contains("username and password"));
    }
}
