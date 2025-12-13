//! Utility functions for processors.

use crate::pipeline::job_queue::{JobLogEntry, LogLevel};
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{debug, warn};

/// Video file extensions that support processing.
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "webm", "mov", "flv", "avi", "wmv", "m4v", "ts", "mts", "m2ts", "3gp", "ogv",
];

/// Audio file extensions.
pub const AUDIO_EXTENSIONS: &[&str] = &["mp3", "aac", "m4a", "ogg", "opus", "flac", "wav"];

/// Image file extensions that should be passed through.
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "tif", "ico", "avif",
];

/// Get the lowercase extension from a path.
pub fn get_extension(path: &str) -> Option<String> {
    Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
}

/// Check if the extension is a video format.
pub fn is_video(ext: &str) -> bool {
    VIDEO_EXTENSIONS.contains(&ext)
}

/// Check if the extension is an audio format.
pub fn is_audio(ext: &str) -> bool {
    AUDIO_EXTENSIONS.contains(&ext)
}

/// Check if the extension is an image format.
pub fn is_image(ext: &str) -> bool {
    IMAGE_EXTENSIONS.contains(&ext)
}

/// Check if the extension is a media format (video or audio).
pub fn is_media(ext: &str) -> bool {
    is_video(ext) || is_audio(ext)
}

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

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    // Handle stdout
    let stdout_handle = if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        Some(tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                debug!("stdout: {}", line);
                let _ = tx.send(create_log_entry(LogLevel::Info, line));
            }
        }))
    } else {
        None
    };

    // Handle stderr
    let stderr_handle = if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        Some(tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // FFmpeg outputs progress to stderr, so we check for error indicators
                // Use more specific patterns to avoid false positives
                let level = if line.starts_with("[error]")
                    || line.contains("Error ")
                    || line.contains("error:")
                    || line.contains("failed")
                    || line.contains("Invalid ")
                {
                    warn!("stderr: {}", line);
                    LogLevel::Error
                } else {
                    debug!("stderr: {}", line);
                    LogLevel::Info
                };

                let _ = tx.send(create_log_entry(level, line));
            }
        }))
    } else {
        None
    };

    // Drop original sender so channel closes when tasks complete
    drop(tx);

    let status = child
        .wait()
        .await
        .map_err(|e| crate::Error::Other(format!("Failed to wait for command: {}", e)))?;

    // Wait for reader tasks to complete to ensure all output is captured
    if let Some(handle) = stdout_handle {
        let _ = handle.await;
    }
    if let Some(handle) = stderr_handle {
        let _ = handle.await;
    }

    let duration = start.elapsed().as_secs_f64();

    // Collect all logs from the channel
    let mut logs = Vec::new();
    while let Some(entry) = rx.recv().await {
        logs.push(entry);
    }

    Ok(CommandOutput {
        status,
        duration,
        logs,
    })
}
