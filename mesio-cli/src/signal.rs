//! Turns termination signals into the same graceful stop as pressing `q`,
//! and, on Unix, `SIGUSR1` into the same cut as pressing `s`.

use std::io;
use std::sync::mpsc;
use std::thread;

use pipeline_common::CancellationToken;
use tracing::info;

use crate::cut::{CutSource, CutTarget};

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
pub fn spawn(token: CancellationToken, cut: CutTarget) -> io::Result<()> {
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
                        handle_signals(signals, token, cut).await;
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

/// What a received signal asks for.
enum Received {
    Stop(&'static str),
    #[cfg_attr(windows, allow(dead_code))]
    Cut,
}

/// Cancels `token` on the first termination signal, so the current file is
/// finished. A termination signal that arrives once a stop is under way (after
/// a signal, `q`, or a closed output pipe) exits immediately. A cut request
/// goes to `cut` and never stops the process.
///
/// Nothing here writes to stderr or flushes stdout, either of which may be a
/// pipe that has stopped draining, which is when a forced exit is most needed.
async fn handle_signals(mut signals: Signals, token: CancellationToken, cut: CutTarget) {
    while let Some(received) = signals.recv().await {
        let signal = match received {
            Received::Stop(signal) => signal,
            // Acknowledged on the cut notice thread, not here.
            Received::Cut => {
                cut.trigger(CutSource::Signal);
                continue;
            }
        };
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
    cut: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Signals {
    fn new() -> io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
            cut: signal(SignalKind::user_defined1())?,
        })
    }

    async fn recv(&mut self) -> Option<Received> {
        tokio::select! {
            received = self.interrupt.recv() => received.map(|()| Received::Stop("SIGINT")),
            received = self.terminate.recv() => received.map(|()| Received::Stop("SIGTERM")),
            received = self.cut.recv() => received.map(|()| Received::Cut),
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

    async fn recv(&mut self) -> Option<Received> {
        tokio::select! {
            received = self.ctrl_c.recv() => received.map(|()| Received::Stop("Ctrl+C")),
            received = self.ctrl_break.recv() => received.map(|()| Received::Stop("Ctrl+Break")),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;
    use std::sync::mpsc::Receiver;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::cut::{CutOutcome, Notice, Unavailable};

    const CHILD_ENV: &str = "MESIO_SIGNAL_TEST_CHILD";
    const TEST_NAME: &str = "signal::tests::a_signal_stops_gracefully_and_a_second_one_at_once";

    fn send_to_self(signal: &str) {
        let status = Command::new("kill")
            .args([signal, &std::process::id().to_string()])
            .status()
            .expect("kill runs");
        assert!(status.success());
    }

    fn next_notice(notices: &Receiver<Notice>) -> Notice {
        notices
            .recv_timeout(Duration::from_secs(5))
            .expect("SIGUSR1 requests a cut")
    }

    /// Runs in a child process: the installed handlers stay for the life of
    /// the process, and the second termination signal exits it. The signals
    /// arrive while the test's own runtime is blocked, as a stalled log write
    /// would block the main runtime. `SIGUSR1` requests a cut, before or
    /// during a stop, and never stops or exits the process.
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
            let (cut, notices) = CutTarget::with_notice_receiver();
            spawn(token.clone(), cut).expect("signal handlers install");

            send_to_self("-USR1");
            let notice = next_notice(&notices);
            assert_eq!(notice.source, CutSource::Signal);
            assert_eq!(
                notice.outcome,
                CutOutcome::Unavailable(Unavailable::NoRecording)
            );
            assert!(!token.is_cancelled(), "a cut request does not stop");

            send_to_self("-TERM");
            let deadline = Instant::now() + Duration::from_secs(5);
            while !token.is_cancelled() {
                assert!(
                    Instant::now() < deadline,
                    "the first signal cancels the token"
                );
                thread::sleep(Duration::from_millis(10));
            }

            // During a stop, a cut request is still only a request.
            send_to_self("-USR1");
            next_notice(&notices);

            send_to_self("-TERM");
            thread::sleep(Duration::from_secs(5));
        });
        panic!("the second signal did not exit the process");
    }
}
