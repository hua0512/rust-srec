//! A loopback SMTP server that accepts every message without TLS or login.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// A message the [`SmtpFixture`] accepted.
#[derive(Debug, Clone, Default)]
pub struct SmtpMessage {
    /// The `MAIL FROM` argument, such as `<srec@example.com>`.
    pub from: String,
    /// The `RCPT TO` arguments.
    pub to: Vec<String>,
    /// The message as sent after `DATA`, headers included, lines joined with
    /// CRLF and without the terminating dot.
    pub data: String,
}

pub struct SmtpFixture {
    port: u16,
    messages: Arc<Mutex<Vec<SmtpMessage>>>,
    task: tokio::task::JoinHandle<()>,
}

impl SmtpFixture {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the SMTP fixture");
        let port = listener.local_addr().expect("SMTP fixture address").port();
        let messages = Arc::new(Mutex::new(Vec::new()));
        let delivered = messages.clone();
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { continue };
                        connections.spawn(serve_connection(stream, delivered.clone()));
                    }
                    Some(_) = connections.join_next(), if !connections.is_empty() => {}
                }
            }
        });
        Self {
            port,
            messages,
            task,
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Messages accepted so far.
    pub fn messages(&self) -> Vec<SmtpMessage> {
        self.messages.lock().expect("SMTP message lock").clone()
    }
}

impl Drop for SmtpFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// One SMTP session. Commands the fixture does not implement get a 502, so
/// the client reports the failure instead of the fixture panicking in a task.
async fn serve_connection(stream: TcpStream, messages: Arc<Mutex<Vec<SmtpMessage>>>) {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    if writer
        .write_all(b"220 localhost SMTP fixture\r\n")
        .await
        .is_err()
    {
        return;
    }
    let mut message = SmtpMessage::default();
    let mut data: Option<Vec<String>> = None;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if let Some(lines) = data.as_mut() {
            if line == ".\r\n" {
                message.data = lines.join("\r\n");
                data = None;
                messages
                    .lock()
                    .expect("SMTP message lock")
                    .push(std::mem::take(&mut message));
                if writer.write_all(b"250 delivered\r\n").await.is_err() {
                    return;
                }
            } else {
                let line = line.trim_end_matches("\r\n");
                // Dot-stuffing: a leading dot was doubled by the client.
                lines.push(line.strip_prefix('.').unwrap_or(line).to_owned());
            }
            continue;
        }
        let command = line.trim_end_matches("\r\n");
        let upper = command.to_ascii_uppercase();
        let response: &[u8] = if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            b"250 localhost\r\n"
        } else if let Some(from) = upper.strip_prefix("MAIL FROM:").map(|_| &command[10..]) {
            message.from = from.trim().to_owned();
            b"250 OK\r\n"
        } else if let Some(to) = upper.strip_prefix("RCPT TO:").map(|_| &command[8..]) {
            message.to.push(to.trim().to_owned());
            b"250 OK\r\n"
        } else if upper == "NOOP" || upper == "RSET" {
            b"250 OK\r\n"
        } else if upper == "DATA" {
            data = Some(Vec::new());
            b"354 End with <CRLF>.<CRLF>\r\n"
        } else if upper == "QUIT" {
            // The client may already have hung up.
            let _ = writer.write_all(b"221 goodbye\r\n").await;
            return;
        } else {
            b"502 command not implemented\r\n"
        };
        if writer.write_all(response).await.is_err() {
            return;
        }
    }
}
