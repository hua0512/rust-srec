//! A private, bounded control connection to the embedded Streamlink companion.

use std::io;
use std::net::Ipv4Addr;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tempfile::TempDir;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::time::{Instant, timeout_at};
use tokio_util::task::AbortOnDropHandle;
use uuid::Uuid;

const SOURCE: &str = include_str!("srec_control.py");
const PROFILE: &str = "streamlink-8.5.0-drain-v1";
const MAX_FRAME: u64 = 4096;

mod cache;

#[derive(Clone, Default)]
struct State {
    ready: bool,
    drained: bool,
    closed: bool,
    failure: Option<String>,
}

#[derive(Deserialize)]
struct Frame {
    token: String,
    protocol: u32,
    profile: String,
    event: String,
    #[serde(default)]
    reason: String,
}

pub(super) struct Companion {
    directory: Arc<TempDir>,
    token: String,
    port: u16,
    stop: watch::Sender<Option<Instant>>,
    state: watch::Receiver<State>,
    _task: AbortOnDropHandle<()>,
}

impl Companion {
    pub(super) async fn cached_capability(binary: &str) -> bool {
        cache::capability(binary).await
    }
    pub(super) fn supports_version(version: Option<&str>) -> bool {
        version.is_some_and(|version| version.split_whitespace().any(|word| word == "8.5.0"))
    }

    /// Establish loader support without changing a recording command. Opaque
    /// executables that reject the plugin option keep their original argv.
    pub(super) async fn probe(binary: &str) -> bool {
        let Ok(control) = Self::prepare().await else {
            return false;
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut command = process_utils::tokio_command(binary);
        control.configure(&mut command);
        command
            .arg("--help")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let Ok(mut child) = process_utils::ContainedChild::spawn(&mut command) else {
            return false;
        };
        match timeout_at(deadline, child.wait()).await {
            Ok(Ok(status)) if status.success() => {
                let mut state = control.state.clone();
                loop {
                    let current = state.borrow_and_update().clone();
                    if current.ready {
                        return true;
                    }
                    if current.failure.is_some() || current.closed {
                        return false;
                    }
                    if !matches!(timeout_at(deadline, state.changed()).await, Ok(Ok(()))) {
                        return false;
                    }
                }
            }
            _ => {
                if let Err(error) = child.terminate_tree_until(deadline).await {
                    tracing::warn!(%error, "Could not settle Streamlink control capability probe");
                }
                false
            }
        }
    }

    pub(super) async fn prepare() -> io::Result<Self> {
        let directory = Arc::new(
            tempfile::Builder::new()
                .prefix("srec-streamlink-")
                .tempdir()?,
        );
        // Plugin names are global across directories. A per-attempt module name
        // cannot shadow an existing user plugin from the default search paths.
        let module = format!("srec_recording_control_{}.py", Uuid::new_v4().simple());
        tokio::fs::write(directory.path().join(module), SOURCE).await?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let (stop, stopping) = watch::channel(None);
        let (updates, state) = watch::channel(State::default());
        let task_token = token.clone();
        let retained_directory = directory.clone();
        let task = AbortOnDropHandle::new(tokio::spawn(async move {
            let _directory = retained_directory;
            if let Err(error) = serve(listener, &task_token, stopping, &updates).await {
                updates.send_modify(|state| {
                    state
                        .failure
                        .get_or_insert_with(|| format!("control connection: {error}"));
                });
            }
            updates.send_modify(|state| state.closed = true);
        }));
        Ok(Self {
            directory,
            token,
            port,
            stop,
            state,
            _task: task,
        })
    }

    pub(super) fn configure(&self, command: &mut tokio::process::Command) {
        command.arg("--plugin-dir").arg(self.directory.path());
        self.configure_environment(command);
    }

    pub(super) fn configure_environment(&self, command: &mut tokio::process::Command) {
        command
            .env("SREC_STREAMLINK_CONTROL_PORT", self.port.to_string())
            .env("SREC_STREAMLINK_CONTROL_TOKEN", &self.token);
    }

    pub(super) async fn request_stop(&self, deadline: Instant) -> bool {
        self.stop.send_replace(Some(deadline));
        let mut state = self.state.clone();
        let handshake_deadline = deadline.min(Instant::now() + Duration::from_millis(500));
        loop {
            let current = state.borrow_and_update().clone();
            if current.closed {
                return false;
            }
            // A live companion can still stop acquisition after reporting a
            // drain-only error. Final completion checks that error separately.
            if current.ready {
                return true;
            }
            if current.failure.is_some() {
                return false;
            }
            if !matches!(
                timeout_at(handshake_deadline, state.changed()).await,
                Ok(Ok(()))
            ) {
                return false;
            }
        }
    }

    pub(super) async fn drain_result(&self, deadline: Instant) -> Result<(), String> {
        let mut state = self.state.clone();
        loop {
            let current = state.borrow_and_update().clone();
            if let Some(error) = current.failure {
                return Err(error);
            }
            if current.drained {
                return Ok(());
            }
            if current.closed {
                return Err("companion exited without a verified drain".into());
            }
            if !matches!(timeout_at(deadline, state.changed()).await, Ok(Ok(()))) {
                return Err("companion drain deadline expired".into());
            }
        }
    }
}

async fn frame<R: AsyncBufRead + Unpin>(reader: &mut R) -> io::Result<Frame> {
    let mut bytes = Vec::new();
    let read = reader
        .take(MAX_FRAME + 1)
        .read_until(b'\n', &mut bytes)
        .await?;
    if read == 0 || read as u64 > MAX_FRAME || bytes.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid control frame length",
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid control message"))
}

fn authentic(frame: &Frame, token: &str) -> bool {
    // Fixed-length tokens are random per attempt and never included in logs.
    frame.protocol == 1
        && frame.profile == PROFILE
        && frame.token.len() == token.len()
        && frame
            .token
            .bytes()
            .zip(token.bytes())
            .fold(0, |diff, (a, b)| diff | (a ^ b))
            == 0
}

async fn serve(
    listener: TcpListener,
    token: &str,
    mut stop: watch::Receiver<Option<Instant>>,
    updates: &watch::Sender<State>,
) -> io::Result<()> {
    // Bound unauthenticated connections and reads; no resource grows with input.
    let mut accepted = None;
    for _ in 0..8 {
        let (connection, _) = listener.accept().await?;
        let (read, write) = connection.into_split();
        let mut reader = BufReader::new(read);
        let first = timeout_at(Instant::now() + Duration::from_secs(2), frame(&mut reader)).await;
        if let Ok(Ok(first)) = first
            && authentic(&first, token)
        {
            accepted = Some((reader, write, first));
            break;
        }
    }
    let Some((mut reader, mut write, first)) = accepted else {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "control authentication failed",
        ));
    };
    let mut sent_stop = false;
    let mut next = Some(first);
    loop {
        if let Some(message) = next.take() {
            if !authentic(&message, token) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "invalid control identity",
                ));
            }
            match message.event.as_str() {
                "ready" => updates.send_modify(|state| state.ready = true),
                "drained" if sent_stop => {
                    updates.send_modify(|state| state.drained = true);
                    return Ok(());
                }
                "unsupported" => {
                    updates.send_modify(|state| {
                        state.failure = Some(message.reason.chars().take(256).collect())
                    });
                    return Ok(());
                }
                "incomplete" => updates.send_modify(|state| {
                    state.failure = Some(message.reason.chars().take(256).collect())
                }),
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "unexpected control state",
                    ));
                }
            }
        }
        // Keep partial input alive across a concurrent stop. Recreating frame()
        // after select cancellation would discard bytes already read from TCP.
        let mut incoming = std::pin::pin!(frame(&mut reader));
        loop {
            let deadline = *stop.borrow_and_update();
            if let Some(deadline) = deadline
                && !sent_stop
            {
                let command = serde_json::json!({
                    "protocol": 1, "token": token, "command": "stop",
                    "budget_ms": deadline.saturating_duration_since(Instant::now()).as_millis().min(86400000),
                });
                let mut data = serde_json::to_vec(&command).map_err(io::Error::other)?;
                data.push(b'\n');
                timeout_at(deadline, write.write_all(&data))
                    .await
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::TimedOut, "stop request expired")
                    })??;
                sent_stop = true;
            }
            next = tokio::select! {
              message = &mut incoming => Some(message?),
              changed = stop.changed(), if !sent_stop => {
                  if changed.is_err() { return Ok(()); }
                  None
              }
            };
            if next.is_some() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests;
