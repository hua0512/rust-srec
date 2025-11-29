//! Email notification channel using SMTP.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::NotificationChannel;
use crate::notification::events::{NotificationEvent, NotificationPriority};
use crate::Result;

/// Email channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailConfig {
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
    /// Batch emails within this window (seconds).
    #[serde(default = "default_batch_window")]
    pub batch_window_secs: u64,
}

fn default_email_priority() -> NotificationPriority {
    NotificationPriority::High
}

fn default_batch_window() -> u64 {
    60
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            smtp_host: "localhost".to_string(),
            smtp_port: 587,
            smtp_username: None,
            smtp_password: None,
            use_tls: true,
            from_address: String::new(),
            to_addresses: Vec::new(),
            min_priority: NotificationPriority::High,
            batch_window_secs: 60,
        }
    }
}

/// Email notification channel.
pub struct EmailChannel {
    config: EmailConfig,
}

impl EmailChannel {
    /// Create a new Email channel.
    pub fn new(config: EmailConfig) -> Self {
        Self { config }
    }

    /// Build the email subject.
    fn build_subject(&self, event: &NotificationEvent) -> String {
        format!("[rust-srec] {}", event.title())
    }

    /// Build the email body (plain text).
    fn build_body_text(&self, event: &NotificationEvent) -> String {
        format!(
            "{}\n\n{}\n\nPriority: {}\nType: {}\nTime: {}",
            event.title(),
            event.description(),
            event.priority(),
            event.event_type(),
            event.timestamp().to_rfc3339()
        )
    }

    /// Build the email body (HTML).
    fn build_body_html(&self, event: &NotificationEvent) -> String {
        let priority_color = match event.priority() {
            NotificationPriority::Low => "#808080",
            NotificationPriority::Normal => "#3498db",
            NotificationPriority::High => "#f39c12",
            NotificationPriority::Critical => "#e74c3c",
        };

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
            priority_color,
            event.title(),
            event.description(),
            event.priority(),
            event.event_type(),
            event.timestamp().to_rfc3339()
        )
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

        // Note: In a real implementation, we would use the `lettre` crate here.
        // For now, we log the email that would be sent.
        let subject = self.build_subject(event);
        let _body_text = self.build_body_text(event);
        let _body_html = self.build_body_html(event);

        // TODO: Implement actual SMTP sending with lettre crate
        // let email = Message::builder()
        //     .from(self.config.from_address.parse()?)
        //     .to(self.config.to_addresses[0].parse()?)
        //     .subject(&subject)
        //     .multipart(MultiPart::alternative_plain_html(body_text, body_html))?;

        warn!(
            "Email sending not yet implemented. Would send: {} to {:?}",
            subject, self.config.to_addresses
        );

        debug!("Email notification prepared: {}", event.event_type());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_email_config_default() {
        let config = EmailConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.smtp_port, 587);
        assert!(config.use_tls);
        assert_eq!(config.min_priority, NotificationPriority::High);
    }

    #[test]
    fn test_email_channel_disabled() {
        let config = EmailConfig::default();
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

        let subject = channel.build_subject(&event);
        assert!(subject.contains("rust-srec"));
        assert!(subject.contains("TestStreamer"));
    }
}
