//! Utility functions for processors.

use crate::pipeline::job_queue::{JobLogEntry, LogLevel};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{debug, error, info, warn};

/// Output from a command execution including captured logs.
pub struct CommandOutput {
    pub status: std::process::ExitStatus,
    pub duration: f64,
    pub logs: Vec<JobLogEntry>,
}

/// Helper function to create a log entry.
pub fn create_log_entry(level: LogLevel, message: impl Into<String>) -> JobLogEntry {
    JobLogEntry::new(level, message)
}

/// Run a command and capture its output (stdout/stderr) as logs.
/// This helper handles spawning the process, reading output streams asynchronously,
/// and collecting them into a structured log format.
pub async fn run_command_with_logs(command: &mut Command) -> crate::Result<CommandOutput> {
    let start = std::time::Instant::now();

    // Ensure pipes are set up
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|e| crate::Error::Other(format!("Failed to spawn command: {}", e)))?;

    // We will collect logs here. We need a way to share this across tasks.
    // Using a channel is a good way to stream logs from async readers.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    // Handle stdout
    if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // Log to tracing for backend visibility
                debug!("stdout: {}", line);
                // Send to channel for persistence
                let _ = tx.send(JobLogEntry::new(LogLevel::Info, line));
            }
        });
    }

    // Handle stderr
    if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // FFmpeg and others often print progress/info to stderr, so we need to be careful
                // not to treat everything as an error. For now, we'll treat it as info/debug
                // unless it explicitly looks like an error, or just log at 'Info' level generally
                // as many tools output normal log data to stderr.
                // However, to match previous behavior, we might check for "error" keywords.

                let level = if line.to_lowercase().contains("error") {
                    warn!("stderr: {}", line);
                    LogLevel::Error
                } else {
                    debug!("stderr: {}", line);
                    LogLevel::Info
                };

                let _ = tx.send(JobLogEntry::new(level, line));
            }
        });
    }

    // Drop the original transmitter so the receiver knows when all senders are done.
    // However, the channel will actually stay open until the child process finishes
    // AND the stream readers finish.
    // We can't easily "wait" for the stream readers unless we keep join handles.
    // But since the stream readers run until EOF (which happens when child closes pipes),
    // waiting for the child usually implies waiting for streams soon after.
    // A better approach specifically for `wait`:
    drop(tx);

    let status = child
        .wait()
        .await
        .map_err(|e| crate::Error::Other(format!("Failed to wait for command: {}", e)))?;

    let duration = start.elapsed().as_secs_f64();

    // Collect all logs from the channel
    let mut logs = Vec::new();
    while let Some(entry) = rx.recv().await {
        logs.push(entry);
    }

    // Sort logs by timestamp slightly? The channel order should be roughly correct,
    // but due to async scheduling it's not guaranteed perfectly strict.
    // Given the timestamp resolution, strict sorting might not change much.

    Ok(CommandOutput {
        status,
        duration,
        logs,
    })
}
