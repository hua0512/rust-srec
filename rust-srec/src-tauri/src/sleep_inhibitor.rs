use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long to wait before retrying after the OS refused an inhibitor while
/// one is still wanted. Callers sync on every progress tick, and on a Linux
/// session without logind or the desktop portal each attempt is a blocking
/// D-Bus call that fails the same way.
const RETRY_BACKOFF: Duration = Duration::from_secs(10 * 60);

fn acquire() -> Result<keepawake::KeepAwake, keepawake::Error> {
    keepawake::Builder::default()
        .idle(true)
        .display(false)
        .reason("Recording active streams")
        .app_name("Rust-Srec")
        .app_reverse_domain("com.hua0512.rust-srec")
        .create()
}

/// Idle-sleep inhibition state owned by the worker thread.
#[derive(Default)]
struct Inhibitor {
    handle: Option<keepawake::KeepAwake>,
    /// When the current run of failed acquisitions last failed. Cleared once
    /// the inhibitor is no longer wanted or is acquired, so the next recording
    /// retries at once and warns again if it still fails.
    failed_at: Option<Instant>,
}

impl Inhibitor {
    fn sync(&mut self, prevent_sleep_enabled: bool, active_recordings: usize) {
        let wanted = prevent_sleep_enabled && active_recordings > 0;
        match (wanted, self.handle.is_some()) {
            (false, held) => {
                self.failed_at = None;
                if held {
                    self.handle = None;
                    log::info!("Released system sleep inhibitor");
                }
            }
            (true, true) => {}
            (true, false) => {
                let now = Instant::now();
                if self
                    .failed_at
                    .is_some_and(|at| now.saturating_duration_since(at) < RETRY_BACKOFF)
                {
                    return;
                }
                match acquire() {
                    Ok(handle) => {
                        self.handle = Some(handle);
                        self.failed_at = None;
                        log::info!(
                            "Acquired system sleep inhibitor ({} active recording(s))",
                            active_recordings
                        );
                    }
                    Err(error) => {
                        if self.failed_at.is_none() {
                            log::warn!("Failed to acquire system sleep inhibitor: {}", error);
                        } else {
                            log::debug!("Retry of system sleep inhibitor failed: {}", error);
                        }
                        self.failed_at = Some(now);
                    }
                }
            }
        }
    }
}

/// Keeps the machine awake while recordings are in progress, from a dedicated
/// OS thread.
///
/// On Windows, `SetThreadExecutionState` tracks `ES_SYSTEM_REQUIRED` strictly
/// per OS thread, so creating and dropping `KeepAwake` across Tokio worker
/// threads would permanently leak the sleep block. On Linux, `keepawake` runs
/// synchronous `zbus::blocking` D-Bus calls that must not block async workers.
/// Driving the inhibitor from one dedicated OS thread satisfies both invariants.
/// Dropping the worker closes the channel, which ends the thread and releases
/// any held inhibitor on it.
pub struct SleepInhibitorWorker {
    tx: Option<mpsc::Sender<(bool, usize)>>,
}

impl SleepInhibitorWorker {
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel::<(bool, usize)>();
        match std::thread::Builder::new()
            .name("sleep-inhibitor".to_string())
            .spawn(move || {
                let mut inhibitor = Inhibitor::default();
                while let Ok(mut latest) = rx.recv() {
                    // Coalesce bursts so only the latest state hits OS power APIs.
                    while let Ok(next) = rx.try_recv() {
                        latest = next;
                    }
                    inhibitor.sync(latest.0, latest.1);
                }
                // Dropping `inhibitor` here guarantees `KeepAwake::drop` runs on
                // the same OS thread that created it.
            }) {
            Ok(_) => Self { tx: Some(tx) },
            Err(error) => {
                log::warn!("Failed to spawn sleep inhibitor thread: {}", error);
                Self { tx: None }
            }
        }
    }

    pub fn sync(&self, prevent_sleep_enabled: bool, active_recordings: usize) {
        if let Some(tx) = &self.tx
            && tx.send((prevent_sleep_enabled, active_recordings)).is_err()
        {
            log::warn!("Sleep inhibitor worker thread stopped unexpectedly");
        }
    }
}
