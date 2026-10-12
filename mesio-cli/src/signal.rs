//! Turns termination signals into the same graceful stop as pressing `q`.

use std::io;

use crossterm::terminal;
use pipeline_common::CancellationToken;
use tracing::{debug, info, warn};

/// Exit status after a forced stop: what a shell reports for a process
/// killed by SIGINT (128 + 2).
const FORCED_EXIT_CODE: i32 = 130;

/// Installs the signal handlers. Until this returns, signals keep their
/// default action and end the process at once.
pub fn install() -> io::Result<Signals> {
    Signals::new()
}

/// Cancels `token` on the first termination signal, so the current file is
/// finished. A signal that arrives once the token is already cancelled
/// (a second signal, or one during a slow stop after `q`) exits immediately.
pub async fn stop_on_signal(mut signals: Signals, token: CancellationToken) {
    while let Some(signal) = signals.recv().await {
        if token.is_cancelled() {
            warn!(
                signal,
                "Stopping immediately; the current file may be incomplete"
            );
            // The terminal input handler restores the terminal only when it
            // gets to run again, which an immediate exit does not allow.
            if let Err(error) = terminal::disable_raw_mode() {
                debug!(%error, "Failed to restore the terminal mode");
            }
            std::process::exit(FORCED_EXIT_CODE);
        }
        info!(
            signal,
            "Finishing the current file before exiting; send the signal again to stop immediately"
        );
        token.cancel();
    }
}

#[cfg(unix)]
pub struct Signals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Signals {
    fn new() -> io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }

    async fn recv(&mut self) -> Option<&'static str> {
        tokio::select! {
            received = self.interrupt.recv() => received.map(|()| "SIGINT"),
            received = self.terminate.recv() => received.map(|()| "SIGTERM"),
        }
    }
}

#[cfg(windows)]
pub struct Signals {
    ctrl_c: tokio::signal::windows::CtrlC,
    ctrl_break: tokio::signal::windows::CtrlBreak,
}

#[cfg(windows)]
impl Signals {
    fn new() -> io::Result<Self> {
        use tokio::signal::windows::{ctrl_break, ctrl_c};
        Ok(Self {
            ctrl_c: ctrl_c()?,
            ctrl_break: ctrl_break()?,
        })
    }

    async fn recv(&mut self) -> Option<&'static str> {
        tokio::select! {
            received = self.ctrl_c.recv() => received.map(|()| "Ctrl+C"),
            received = self.ctrl_break.recv() => received.map(|()| "Ctrl+Break"),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn a_first_signal_cancels_the_token() {
        let signals = install().expect("signal handlers install");
        let token = CancellationToken::new();
        tokio::spawn(stop_on_signal(signals, token.clone()));

        let status = Command::new("kill")
            .args(["-TERM", &std::process::id().to_string()])
            .status()
            .expect("kill runs");
        assert!(status.success());

        tokio::time::timeout(Duration::from_secs(5), token.cancelled())
            .await
            .expect("the token is cancelled");
    }
}
