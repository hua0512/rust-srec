//! Service layer module.
//!
//! This module provides the service container, startup data upgrades,
//! database maintenance scheduling, and service-level adapters.

pub(crate) mod baidupcs_auth;
pub(crate) mod config_export;
pub(crate) mod config_import;
pub(crate) mod container;
#[doc(hidden)]
pub mod legacy_upgrade;
#[doc(hidden)]
pub mod maintenance;
pub(crate) mod notification_bridge;
pub(crate) mod pipeline_ports;
pub(crate) mod playback_context;

pub use container::{ActiveRecordingSummary, ServiceContainer, ServiceContainerConfig};
