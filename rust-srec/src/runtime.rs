//! Process-isolated runtime supervision.

mod deadline;
mod generation;
mod supervisor;

pub use generation::RuntimeLease;
pub(crate) use supervisor::{
    RuntimeTermination, WorkerShutdownReason, is_worker_process, supervise_current_executable,
    wait_for_worker_start, worker_shutdown_schedule,
};
