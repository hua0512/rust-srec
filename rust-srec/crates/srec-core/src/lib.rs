//! Storage-free domain layer, shared contracts, and core utilities for `rust-srec`.

pub mod domain;
pub mod error;
pub mod utils;

pub use error::{CoreError, CoreResult};
