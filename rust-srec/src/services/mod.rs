//! Service layer module.
//!
//! This module provides the service container and initialization logic
//! for all application services.

pub mod container;
pub mod session_janitor;

pub use container::{ServiceContainer, ServiceStats};
pub use session_janitor::SessionJanitor;
