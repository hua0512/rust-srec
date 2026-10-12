//! Turns termination signals into the same graceful stop as pressing `q`.

use std::io;
use std::sync::mpsc;
use std::thread;

use pipeline_common::CancellationToken;
use tracing::info;

/// Exit status after a forced stop: what a shell reports for a process
/// killed by SIGINT (128 + 2).
const FORCED_EXIT_CODE: i32 = 130;

/// Starts handling termination signals on a thread with a runtime of its
/// own, and returns once the handlers are installed; until then, signals keep
/// their default action and end the process at once.
///
/// The forced exit must not wait for a worker of the main runtime: they may
/// all be blocked, for example writing logs to a pipe that has stopped
/// draining. The thread runs until the process exits.
pub fn spawn(token: CancellationToken) -> io::Result<()> {
    let (installed_tx, installed_rx) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("mesio-signal".to_owned())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    // The receiver only goes away once `spawn` has returned.
                    let _ = installed_tx.send(Err(error));
                    return;
                }
            };
            runtime.block_on(async move {
                match Signals::new() {
                    Ok(signals) => {
                        let _ = installed_tx.send(Ok(()));
                        stop_on_signal(signals, token).await;
                    }
                    Err(error) => {
                        let _ = installed_tx.send(Err(error));
                    }
                }
            });
        })?;
    installed_rx.recv().unwrap_or_else(|_| {
        Err(io::Error::other(
            "the signal thread ended before installing handlers",
        ))
    })
}

/// Cancels `token` on the first termination signal, so the current file is
/// finished. A signal that arrives once a stop is under way (after a signal,
/// `q`, or a closed output pipe) exits immediately.
///
/// Nothing here writes to stderr or flushes stdout, either of which may be a
/// pipe that has stopped draining, which is when a forced exit is most needed.
async fn stop_on_signal(mut signals: Signals, token: CancellationToken) {
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
struct Signals {
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
struct Signals {
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
    use std::time::{Duration, Instant};

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
    /// the process, and the second signal exits it. Both signals arrive while
    /// the test's own runtime is blocked, as a stalled log write would block
    /// the main runtime.
    #[test]
    fn a_signal_stops_gracefully_and_a_second_one_at_once() {
        if std::env::var_os(CHILD_ENV).is_none() {
            let status = Command::new(std::env::current_exe().expect("test binary path"))
                .args([TEST_NAME, "--exact", "--nocapture"])
                .env(CHILD_ENV, "1")
                .status()
                .expect("child test runs");
            assert_eq!(status.code(), Some(FORCED_EXIT_CODE));
            return;
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime builds");
        runtime.block_on(async {
            let token = CancellationToken::new();
            spawn(token.clone()).expect("signal handlers install");

            send_sigterm_to_self();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !token.is_cancelled() {
                assert!(
                    Instant::now() < deadline,
                    "the first signal cancels the token"
                );
                thread::sleep(Duration::from_millis(10));
            }

            send_sigterm_to_self();
            thread::sleep(Duration::from_secs(5));
        });
        panic!("the second signal did not exit the process");
    }
}
