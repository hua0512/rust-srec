//! Utility modules for download engines.

use tokio::sync::mpsc;
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
