//! Domain layer for rust-srec.
//!
//! This module contains the core business logic, entities, value objects, and
//! storage-free contracts shared across layers.

pub mod credential;
pub mod engine_config;
pub mod filename;
pub mod filter;
pub mod notification;
pub mod pipeline_notification;
pub mod processor;
pub mod proxy;
pub mod session;
pub mod step_routes;
pub mod streamer;
pub mod value_objects;

/// Longest username accepted during login and configuration import, counted in
/// `char`s so the bound is identical for ASCII and CJK names.
pub const MAX_USERNAME_LENGTH: usize = 128;

pub use streamer::{Streamer, StreamerState};
pub use value_objects::*;
