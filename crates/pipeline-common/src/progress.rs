use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// A struct to hold progress information.
#[derive(Debug, Clone)]
pub struct Progress {
    /// The number of bytes written to the current file.
    pub bytes_written: u64,
    /// The total number of bytes for the current file (if known).
    pub total_bytes: Option<u64>,
    /// The number of items processed (e.g., tags, segments).
    pub items_processed: u64,
    /// The current write rate in bytes per second.
    pub rate: f64,
    /// The duration of the media processed so far.
    pub duration: Option<Duration>,
}

/// An enum to represent different progress events.
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    /// Indicates that a new file has been opened for writing.
    FileOpened {
        /// The path to the file that was opened.
        path: PathBuf,
    },
    /// An update on the progress of writing to a file.
    ProgressUpdate {
        /// The path to the file being updated.
        path: PathBuf,
        /// The progress data.
        progress: Progress,
    },
    /// Indicates that a file has been closed after writing.
    FileClosed {
        /// The path to the file that was closed.
        path: PathBuf,
    },
}

/// A callback function for progress updates.
pub type OnProgress = Arc<dyn Fn(ProgressEvent) + Send + Sync>;
