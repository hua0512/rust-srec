//! Process management utilities for download engines.
//!
//! Provides abstractions for spawning and managing child processes
//! with cancellation support.

use tokio::process::Child;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};

/// Spawn a task that waits for a process to exit and sends the result
/// through a oneshot channel.
///
/// If the cancellation token is triggered, the process is killed and
/// `None` is sent through the channel.
///
/// # Arguments
/// * `child` - The child process to wait for
/// * `cancellation_token` - Token to signal cancellation
///
/// # Returns
/// A receiver that will receive:
/// * `Some(exit_code)` - If the process exited normally
/// * `None` - If the process was cancelled
pub fn spawn_process_waiter(
    mut child: Child,
    cancellation_token: CancellationToken,
) -> oneshot::Receiver<Option<i32>> {
    let (tx, rx) = oneshot::channel();

    tokio::spawn(async move {
        let exit_code = tokio::select! {
            _ = cancellation_token.cancelled() => {
                let _ = child.kill().await;
                None
            }
            status = child.wait() => {
                match status {
                    Ok(exit_status) => {
                        let code = exit_status.code();
                        if let Some(c) = code {
                            if c != 0 {
                                warn!("Process exited with code: {}", c);
                            }
                        }
                        code
                    }
                    Err(e) => {
                        error!("Error waiting for process: {}", e);
                        Some(-1)
                    }
                }
            }
        };
        let _ = tx.send(exit_code);
    });

    rx
}

/// Spawn a task that waits for two processes (piped) to exit.
///
/// Waits for the first process to finish, then waits for the second
/// and returns its exit code. This is useful for piped processes where
/// the first process produces output consumed by the second.
///
/// # Arguments
/// * `first` - The first (producer) process
/// * `second` - The second (consumer) process
/// * `cancellation_token` - Token to signal cancellation
///
/// # Returns
/// A receiver that will receive:
/// * `Some(exit_code)` - The exit code of the second process
/// * `None` - If the processes were cancelled
pub fn spawn_piped_process_waiter(
    mut first: Child,
    mut second: Child,
    cancellation_token: CancellationToken,
) -> oneshot::Receiver<Option<i32>> {
    let (tx, rx) = oneshot::channel();

    tokio::spawn(async move {
        let exit_code = tokio::select! {
            _ = cancellation_token.cancelled() => {
                let _ = first.kill().await;
                let _ = second.kill().await;
                None
            }
            result = async {
                let _ = first.wait().await;
                second.wait().await
            } => {
                match result {
                    Ok(exit_status) => {
                        let code = exit_status.code();
                        if let Some(c) = code {
                            if c != 0 {
                                warn!("Process exited with code: {}", c);
                            }
                        }
                        code
                    }
                    Err(e) => {
                        error!("Error waiting for process: {}", e);
                        Some(-1)
                    }
                }
            }
        };
        let _ = tx.send(exit_code);
    });

    rx
}
