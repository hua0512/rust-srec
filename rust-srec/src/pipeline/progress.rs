//! Job progress snapshot and reporter types.
//!
//! Canonical types live in [`crate::domain::processor`].

pub(crate) use crate::domain::processor::progress_file_name;
pub use crate::domain::processor::{
    JobPhase, JobProgressSnapshot, JobProgressUpdate, ProgressKind, ProgressReporter,
};
