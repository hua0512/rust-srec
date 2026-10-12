//! The segment engine core: identity, lifecycle state, scheduling, byte
//! budgets, fetch-and-process, and ordered assembly of downloaded segments.
//!
//! See `crates/mesio/docs/HLS_ENGINE_ARCHITECTURE.md` for the design.

pub mod assembler;
pub mod budget;
pub mod config;
pub mod container;
pub mod control;
pub mod crypto;
pub mod descriptor;
pub mod fetch;
pub mod identity;
pub mod input;
pub mod metrics;
pub mod payload;
pub mod reactor;
pub(crate) mod refresh;
pub mod store;
