//! Turns termination signals into the same graceful stop as pressing `q`.

use std::io;
use std::thread;

use pipeline_common::CancellationToken;
use tracing::info;

/// Exit status after a forced stop: what a shell reports for a process
/// killed by SIGINT (128 + 2).
const FORCED_EXIT_CODE: i32 = 130;

/// Installs the signal handlers. Until this returns, signals keep their
/// default action and end the process at once.
pub fn install() -> io::Result<Signals> {
    Signals::new()
}

/// Cancels `token` on the first termination signal, so the current file is
/// finished. A signal that arrives once a stop is under way (after a signal,
/// `q`, or a closed output pipe) exits immediately.
///
/// Nothing here writes to stderr or flushes stdout, either of which may be a
/// pipe that has stopped draining, which is when a forced exit is most needed.
pub async fn stop_on_signal(mut signals: Signals, token: CancellationToken) {
    while let Some(signal) = signals.recv().await {
        if token.is_cancelled() {
            // Exiting matters more than reporting a failure to restore.
            let _ = crate::input::restore_terminal();
            exit_now(FORCED_EXIT_CODE);
        }
        token.cancel();
        // The notice is optional, so it is dropped if no thread can log it.
        let _ = thread::Builder::new()
            .name("mesio-signal-log".to_owned())
            .spawn(move || {
                info!(
                    signal,
                    "Finishing the current file before exiting; send the signal again to stop immediately"
                );
            });
    }
}

/// Gives SIGINT and SIGTERM back their default action of ending the process,
/// for when the runtime that handled them has shut down.
pub fn restore_default_actions() {
    #[cfg(unix)]
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: `SIG_DFL` is a valid disposition for both signals, and
        // nothing handles them once the runtime has shut down.
        unsafe { libc::signal(signal, libc::SIG_DFL) };
    }
}

/// Ends the process without flushing stdio or running exit handlers.
#[cfg(unix)]
fn exit_now(code: i32) -> ! {
    // SAFETY: `_exit` only terminates the process; skipping the stdio flush
    // and exit handlers is the point.
    unsafe { libc::_exit(code) }
}

#[cfg(windows)]
fn exit_now(code: i32) -> ! {
    // SAFETY: `ExitProcess` only terminates the process; it bypasses Rust's
    // stdout flush, which is the point.
    unsafe { windows_sys::Win32::System::Threading::ExitProcess(code.unsigned_abs()) }
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

    const CHILD_ENV: &str = "MESIO_SIGNAL_TEST_CHILD";
    const TEST_NAME: &str = "signal::tests::a_signal_stops_gracefully_and_a_second_one_at_once";

    fn send_sigterm_to_self() {
        let status = Command::new("kill")
            .args(["-TERM", &std::process::id().to_string()])
            .status()
            .expect("kill runs");
        assert!(status.success());
    }

    /// Runs in a child process: the installed handlers stay for the life of
    /// the process, and the second signal exits it.
    #[tokio::test]
    async fn a_signal_stops_gracefully_and_a_second_one_at_once() {
        if std::env::var_os(CHILD_ENV).is_none() {
            let status = Command::new(std::env::current_exe().expect("test binary path"))
                .args([TEST_NAME, "--exact", "--nocapture"])
                .env(CHILD_ENV, "1")
                .status()
                .expect("child test runs");
            assert_eq!(status.code(), Some(FORCED_EXIT_CODE));
            return;
        }

        let signals = install().expect("signal handlers install");
        let token = CancellationToken::new();
        tokio::spawn(stop_on_signal(signals, token.clone()));

        send_sigterm_to_self();
        tokio::time::timeout(Duration::from_secs(5), token.cancelled())
            .await
            .expect("the first signal cancels the token");

        send_sigterm_to_self();
        tokio::time::sleep(Duration::from_secs(5)).await;
        panic!("the second signal did not exit the process");
    }
}
