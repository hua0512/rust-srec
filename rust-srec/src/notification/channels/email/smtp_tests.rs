use std::collections::HashSet;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::task::AbortOnDropHandle;

use super::*;

struct SmtpFixture {
    port: u16,
    deliveries: mpsc::UnboundedReceiver<usize>,
    _task: AbortOnDropHandle<()>,
}

impl SmtpFixture {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, deliveries) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            let mut next_id = 0;
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (stream, _) = accepted.unwrap();
                        next_id += 1;
                        connections.spawn(serve_connection(stream, next_id, tx.clone()));
                    }
                    result = connections.join_next(), if !connections.is_empty() => {
                        result.unwrap().unwrap();
                    }
                }
            }
        });
        Self {
            port,
            deliveries,
            _task: AbortOnDropHandle::new(task),
        }
    }

    fn config(&self) -> EmailConfig {
        EmailConfig {
            enabled: true,
            smtp_host: "127.0.0.1".into(),
            smtp_port: self.port,
            use_tls: false,
            from_address: "sender@example.com".into(),
            to_addresses: vec!["recipient@example.com".into()],
            min_priority: NotificationPriority::Low,
            locale: Some("en".into()),
            ..Default::default()
        }
    }

    async fn send(&mut self, channel: &EmailChannel) -> usize {
        channel.send(&event()).await.unwrap();
        self.deliveries.recv().await.unwrap()
    }

    async fn send_until_reused(&mut self, channel: &EmailChannel) -> HashSet<usize> {
        let mut used = HashSet::new();
        loop {
            let id = self.send(channel).await;
            if !used.insert(id) {
                return used;
            }
            // Lettre recycles through a spawned task. Reuse is eventual, not
            // guaranteed on the very next send; the scenario timeout bounds this loop.
            tokio::task::yield_now().await;
        }
    }
}

async fn serve_connection(stream: TcpStream, id: usize, deliveries: mpsc::UnboundedSender<usize>) {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    writer
        .write_all(b"220 localhost SMTP fixture\r\n")
        .await
        .unwrap();
    let mut data = false;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await.unwrap() == 0 {
            return;
        }
        if data {
            if line == ".\r\n" {
                data = false;
                deliveries.send(id).unwrap();
                writer.write_all(b"250 delivered\r\n").await.unwrap();
            }
            continue;
        }
        let response: &[u8] = if line.starts_with("EHLO") || line.starts_with("HELO") {
            b"250 localhost\r\n"
        } else if line.starts_with("MAIL FROM:")
            || line.starts_with("RCPT TO:")
            || line == "NOOP\r\n"
        {
            b"250 OK\r\n"
        } else if line == "DATA\r\n" {
            data = true;
            b"354 End with dot\r\n"
        } else if line == "QUIT\r\n" {
            writer.write_all(b"221 goodbye\r\n").await.unwrap();
            return;
        } else {
            panic!("unexpected SMTP command: {line}");
        };
        writer.write_all(response).await.unwrap();
    }
}

fn event() -> NotificationEvent {
    NotificationEvent::SystemStartup {
        version: "test".into(),
        timestamp: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn smtp_connections_are_reused_and_replacements_keep_separate_pools() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut fixture = SmtpFixture::start().await;
        let old = EmailChannel::new(fixture.config());
        let old_connections = fixture.send_until_reused(&old).await;

        // Reload builds a new immutable channel. Work admitted against the
        // old channel must remain usable and retain its original transport.
        let replacement = EmailChannel::new(fixture.config());
        let replacement_connections = fixture.send_until_reused(&replacement).await;
        assert!(old_connections.is_disjoint(&replacement_connections));
        let retained_connections = fixture.send_until_reused(&old).await;
        assert!(retained_connections.is_disjoint(&replacement_connections));
        assert!(!retained_connections.is_disjoint(&old_connections));
    })
    .await
    .expect("local SMTP scenario must finish");
}

#[tokio::test]
async fn filtered_sends_do_not_initialize_transport_and_bad_credentials_fail_locally() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut fixture = SmtpFixture::start().await;
        for disabled in [true, false] {
            let mut config = fixture.config();
            config.enabled = !disabled;
            config.min_priority = NotificationPriority::Critical;
            let channel = EmailChannel::new(config);
            channel.send(&event()).await.unwrap();
            assert!(channel.transport.get().is_none());
        }
        let mut config = fixture.config();
        config.smtp_username = Some("user".into());
        let channel = EmailChannel::new(config);
        for _ in 0..2 {
            let error = channel.send(&event()).await.unwrap_err();
            assert!(error.to_string().contains("username and password"));
            assert!(channel.transport.get().is_none());
        }
        assert!(fixture.deliveries.try_recv().is_err());
    })
    .await
    .expect("local validation must finish");
}

#[test]
fn obsolete_batch_window_is_ignored_when_loading_old_config() {
    let mut value = serde_json::to_value(EmailConfig::default()).unwrap();
    value["batch_window_secs"] = serde_json::json!(60);
    let config: EmailConfig = serde_json::from_value(value).unwrap();
    assert!(
        serde_json::to_value(config)
            .unwrap()
            .get("batch_window_secs")
            .is_none()
    );
}
