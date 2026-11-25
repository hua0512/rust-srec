//! Job progress snapshot and reporter types.
//!
//! Canonical types live in [`crate::domain::processor`].

pub use crate::domain::processor::{
    JobPhase, JobProgressSnapshot, JobProgressUpdate, ProgressKind, ProgressReporter,
};
