//! Scheduler module for orchestrating monitoring tasks.
//!
//! The Scheduler is responsible for:
//! - Spawning monitoring tasks for all active streamers
//! - Reacting to configuration update events dynamically
//! - Grouping streamers by platform for batch detection
//! - Managing task lifecycle with structured concurrency
//! - Implementing graceful shutdown
//! - Monitoring system resources (disk space)

mod batch;
mod resource;
mod service;
mod task;

pub use batch::BatchGroup;
pub use resource::{DiskInfo, DiskSpaceStatus, ResourceMonitor};
pub use service::{Scheduler, SchedulerConfig};
pub use task::{MonitoringTask, TaskHandle, TaskStatus};
