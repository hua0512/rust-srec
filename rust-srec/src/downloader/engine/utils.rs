//! Utility modules for download engines.

use tokio::process::Child;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tracing::debug;

use super::traits::SegmentEvent;

mod disk_full;
mod ffmpeg_parser;
mod files;
mod output_record_reader;
mod process_runner;

pub use disk_full::is_disk_full_line;
pub use ffmpeg_parser::{
    is_segment_start, parse_bitrate, parse_opened_path, parse_progress, parse_size, parse_speed,
    parse_time, parse_time_field,
};
pub use files::ensure_output_dir;
pub use output_record_reader::OutputRecordReader;
pub use process_runner::{spawn_piped_process_waiter, spawn_process_waiter};

pub(super) fn observe_segment_event_send(
    result: Result<(), mpsc::error::SendError<SegmentEvent>>,
    streamer_id: &str,
) {
    if let Err(error) = result {
        debug!(%error, %streamer_id, "segment event receiver closed");
    }
}

/// Window an engine allows for reaping a child it has just killed.
///
/// Only covers the kill-and-reap itself, not any graceful exit the engine asked
/// for first — that budget comes from `DownloadHandle::graceful_stop_budget`.
pub(super) const PROCESS_CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);

/// Window an engine allows for its own reader/event tasks to settle after the
/// child process is gone.
pub(super) const TASK_SETTLEMENT_TIMEOUT: Duration = Duration::from_secs(2);

/// Kill `child` and reap it within `timeout`, returning its exit code.
///
/// `Err` means the process could not be proven reaped, so the caller must not
/// report a confirmed cleanup; `process_name` only labels the message.
pub(super) async fn terminate_and_reap(
    child: &mut Child,
    process_name: &str,
    timeout: Duration,
) -> Result<Option<i32>, String> {
    if let Err(error) = child.start_kill() {
        // `start_kill` fails once the child has already exited, so a status is
        // still the successful outcome here.
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.code()),
            Ok(None) => return Err(format!("failed to kill {process_name}: {error}")),
            Err(wait_error) => {
                return Err(format!(
                    "failed to kill {process_name}: {error}; status check failed: {wait_error}"
                ));
            }
        }
    }

    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => Ok(status.code()),
        Ok(Err(error)) => Err(format!("failed to reap {process_name}: {error}")),
        Err(_) => Err(format!("timed out reaping {process_name}")),
    }
}
